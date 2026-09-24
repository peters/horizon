//! Background refresh of a bound worker's billing for its total cost since
//! creation. Settings, credentials and provider I/O stay on a short-lived
//! thread per refresh; the caller only polls.
use super::{
    CreateState,
    cost::{self, Billing, TotalCost},
    progress,
    settings::Settings,
    state::Deployment,
};
pub use horizon_cloud::runpod::billing::{BillingBucket, BucketSize};
use horizon_cloud::{
    Cancellation, CloudError, Worker,
    runpod::{REQUEST_TIMEOUT, RunPod},
};
use std::{
    fmt::Write,
    path::Path,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant, SystemTime},
};
use time::OffsetDateTime;

/// Billing arrives in hourly buckets, so refreshing faster would show little new.
pub const REFRESH_INTERVAL: Duration = Duration::from_mins(5);
/// How far back a total reads; [`COVERAGE`] names this window.
pub const HISTORY: Duration = Duration::from_hours(365 * 24);
/// A refresh makes two provider requests, each bounded by the provider timeout.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(REQUEST_TIMEOUT.as_secs() * 2 + 5);
const COVERAGE: &str = "Compute and the worker's disk as billed by RunPod, plus the hourly rate for running time its billing has not reached yet. Network volume storage is not included because RunPod does not report it per volume, and neither are charges older than one year.";

/// The refresh work, so callers can substitute the provider.
pub type Fetch = fn(&Path, &str, SystemTime, &Cancellation) -> Result<Vec<BillingBucket>, BillingError>;

/// Short reasons, shown as `Total unavailable: <reason>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BillingError {
    #[error("cloud settings are unavailable")]
    Settings,
    #[error("the RunPod API key is unavailable")]
    Credential,
    #[error("RunPod rejected the API key")]
    Unauthorized,
    #[error("RunPod is unreachable")]
    Unreachable,
    #[error("RunPod did not answer in time")]
    TimedOut,
    #[error("RunPod returned HTTP {0}")]
    Http(u16),
    #[error("RunPod rejected the billing request")]
    Rejected,
    #[error("RunPod returned invalid billing")]
    InvalidResponse,
    #[error("billing could not be read")]
    Unavailable,
}

impl From<CloudError> for BillingError {
    fn from(error: CloudError) -> Self {
        match error {
            CloudError::Unauthorized => Self::Unauthorized,
            CloudError::Transport => Self::Unreachable,
            CloudError::Http(status, _) => Self::Http(status),
            CloudError::Rejected(_) => Self::Rejected,
            CloudError::InvalidResponse => Self::InvalidResponse,
            _ => Self::Unavailable,
        }
    }
}

/// Reads worker `pod_id`'s billing for the last [`HISTORY`]: day buckets until
/// the start of the current UTC day and hour buckets since.
/// # Errors
/// Reports unreadable settings or credentials and provider failures.
pub fn fetch(
    settings: &Path,
    pod_id: &str,
    now: SystemTime,
    cancel: &Cancellation,
) -> Result<Vec<BillingBucket>, BillingError> {
    let settings = Settings::load(settings).map_err(|_| BillingError::Settings)?;
    let provider = RunPod::new(settings.credential().map_err(|_| BillingError::Credential)?);
    let today = start_of_day(now).ok_or(BillingError::Unavailable)?;
    let first = today.checked_sub(HISTORY).ok_or(BillingError::Unavailable)?;
    let days = provider.billing(pod_id, BucketSize::Day, first, today, cancel)?;
    let hours = provider.billing(pod_id, BucketSize::Hour, today, now, cancel)?;
    Ok(combine(days, hours, today))
}

/// A window end may be inclusive, so each granularity keeps only its own span
/// and no charge is read twice.
fn combine(days: Vec<BillingBucket>, hours: Vec<BillingBucket>, today: SystemTime) -> Vec<BillingBucket> {
    let starts = |bucket: &BillingBucket| cost::started_at(&bucket.time);
    days.into_iter()
        .filter(|bucket| starts(bucket).is_some_and(|start| start < today))
        .chain(
            hours
                .into_iter()
                .filter(|bucket| starts(bucket).is_some_and(|start| start >= today)),
        )
        .collect()
}

fn start_of_day(now: SystemTime) -> Option<SystemTime> {
    let seconds = now.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let day = BucketSize::Day.duration().as_secs();
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds - seconds % day))
}

/// Billing as last read successfully.
#[derive(Debug)]
pub struct Sample {
    pub buckets: Vec<BillingBucket>,
    /// The buckets reduced once, for totals on every frame.
    pub billing: Billing,
    pub fetched: Instant,
}

/// The last billing of one bound worker and its bounded background refresh.
#[derive(Default)]
pub struct BillingMonitor {
    pod_id: Option<String>,
    pending: Option<Pending>,
    last_attempt: Option<Instant>,
    sample: Option<Sample>,
    error: Option<BillingError>,
}

struct Pending {
    receiver: Receiver<Result<Vec<BillingBucket>, BillingError>>,
    cancel: Cancellation,
    started: Instant,
}

/// A refresh nobody waits for any more stops before its next provider request.
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl BillingMonitor {
    /// Keeps the billing of the deployment's bound worker fresh and forgets it
    /// once no worker is bound. `fetch` reads the settings under `cloud_root` on
    /// its own thread, and `notify` runs once it has finished.
    pub fn follow<N: Fn() + Clone + Send + 'static>(
        &mut self,
        deployment: Option<&Deployment>,
        cloud_root: Option<&Path>,
        fetch: Fetch,
        notify: &N,
    ) {
        let pod = deployment.and_then(|deployment| match &deployment.operation {
            CreateState::Bound { worker_id } => Some(worker_id.as_str()),
            _ => None,
        });
        let (Some(pod), Some(root)) = (pod, cloud_root) else {
            self.stop();
            return;
        };
        self.update(pod, Instant::now(), || {
            let (settings, pod, notify) = (root.join("settings.json"), pod.to_owned(), notify.clone());
            move |cancel: &Cancellation| {
                let result = fetch(&settings, &pod, SystemTime::now(), cancel);
                notify();
                result
            }
        });
    }

    /// Consumes a finished refresh and starts the next once it is due. `job` is
    /// only called to start a refresh.
    fn update<F>(&mut self, pod_id: &str, now: Instant, job: impl FnOnce() -> F)
    where
        F: FnOnce(&Cancellation) -> Result<Vec<BillingBucket>, BillingError> + Send + 'static,
    {
        self.select(pod_id);
        self.poll(now);
        if self.pending.is_some()
            || self
                .last_attempt
                .is_some_and(|at| now.saturating_duration_since(at) < REFRESH_INTERVAL)
        {
            return;
        }
        self.last_attempt = Some(now);
        let job = job();
        let cancel = Cancellation::default();
        let observed = cancel.clone();
        let (sender, receiver) = channel();
        match std::thread::Builder::new().name("cloud-billing".into()).spawn(move || {
            let _ = sender.send(job(&observed));
        }) {
            Ok(_) => {
                self.pending = Some(Pending {
                    receiver,
                    cancel,
                    started: now,
                });
            }
            Err(_) => self.error = Some(BillingError::Unavailable),
        }
    }

    fn poll(&mut self, now: Instant) {
        let Some(pending) = &self.pending else { return };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) if now.saturating_duration_since(pending.started) < REFRESH_TIMEOUT => return,
            Err(TryRecvError::Empty) => Err(BillingError::TimedOut),
            Err(TryRecvError::Disconnected) => Err(BillingError::Unavailable),
        };
        self.pending = None;
        self.store(result, now);
    }

    /// Stores a finished refresh of `pod_id`. A failure keeps the last billing.
    pub fn record(&mut self, pod_id: &str, result: Result<Vec<BillingBucket>, BillingError>, now: Instant) {
        self.select(pod_id);
        self.store(result, now);
    }

    fn select(&mut self, pod_id: &str) {
        if self.pod_id.as_deref() != Some(pod_id) {
            self.stop();
            self.pod_id = Some(pod_id.to_owned());
        }
    }

    fn store(&mut self, result: Result<Vec<BillingBucket>, BillingError>, now: Instant) {
        match result {
            Ok(buckets) => {
                self.sample = Some(Sample {
                    billing: Billing::new(&buckets),
                    buckets,
                    fetched: now,
                });
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Forgets the worker's billing and cancels a running refresh.
    pub fn stop(&mut self) {
        if self.pod_id.is_some() || self.pending.is_some() {
            *self = Self::default();
        }
    }

    #[must_use]
    pub fn refreshing(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub fn error(&self) -> Option<BillingError> {
        self.error
    }

    #[must_use]
    pub fn sample(&self) -> Option<&Sample> {
        self.sample.as_ref()
    }

    /// The worker's cost since creation once its billing has been read.
    #[must_use]
    pub fn total(&self, worker: &Worker, now: SystemTime) -> Option<TotalCost> {
        self.sample.as_ref().map(|sample| sample.billing.total(worker, now))
    }

    /// What a total from this billing covers, how fresh it is and why the last
    /// refresh failed, if it did.
    #[must_use]
    pub fn explanation(&self, total: &TotalCost, now: Instant) -> String {
        let mut text = String::from(COVERAGE);
        if let Some(through) = total.billed_through.and_then(utc_minute) {
            let _ = write!(text, "\nBilled through {through}.");
        }
        if let Some(sample) = &self.sample {
            let age = progress::duration(now.saturating_duration_since(sample.fetched));
            let _ = write!(text, "\nRefreshed {age} ago.");
        }
        if let Some(error) = self.error {
            let _ = write!(text, "\nLast refresh failed: {error}.");
        }
        text
    }
}

fn utc_minute(at: SystemTime) -> Option<String> {
    let since_epoch = time::Duration::try_from(at.duration_since(SystemTime::UNIX_EPOCH).ok()?).ok()?;
    let at = OffsetDateTime::UNIX_EPOCH.checked_add(since_epoch)?;
    Some(format!(
        "{}-{:02}-{:02} {:02}:{:02} UTC",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    type Result = std::result::Result<Vec<BillingBucket>, BillingError>;

    fn bucket(time: &str, size: BucketSize, amount: f64) -> BillingBucket {
        BillingBucket {
            time: time.into(),
            size,
            amount,
            time_billed_ms: 0,
        }
    }

    fn at(time: &str) -> SystemTime {
        SystemTime::from(OffsetDateTime::parse(time, &time::format_description::well_known::Rfc3339).unwrap())
    }

    /// Polls until the refresh thread has answered; the clock does not advance.
    fn settle(monitor: &mut BillingMonitor, now: Instant) {
        for _ in 0..500 {
            monitor.poll(now);
            if !monitor.refreshing() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the refresh did not finish");
    }

    fn answer(result: Result) -> impl FnOnce() -> Box<dyn FnOnce(&Cancellation) -> Result + Send> {
        move || Box::new(move |_: &Cancellation| result)
    }

    fn deployment(operation: &serde_json::Value) -> Deployment {
        serde_json::from_value(json!({
            "version":1,"cloud_id":"billing-fixture","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Ready","operation":operation,"spec":null,"worker":null,"sessions":[]
        }))
        .unwrap()
    }

    #[test]
    fn refreshes_keep_the_last_good_billing_and_report_the_last_error() {
        let mut monitor = BillingMonitor::default();
        let start = Instant::now();
        let first = vec![bucket("2024-07-12T19:00:00Z", BucketSize::Hour, 0.25)];
        monitor.update("worker1", start, answer(Ok(first.clone())));
        assert!(monitor.refreshing());
        settle(&mut monitor, start);
        assert_eq!(monitor.sample().unwrap().buckets, first);
        assert_eq!(monitor.error(), None);

        let starts = AtomicUsize::new(0);
        let counted = || {
            starts.fetch_add(1, Ordering::Relaxed);
            answer(Err(BillingError::Unreachable))()
        };
        monitor.update("worker1", start + REFRESH_INTERVAL / 2, counted);
        assert_eq!(starts.load(Ordering::Relaxed), 0, "no refresh before the interval");
        let later = start + REFRESH_INTERVAL;
        monitor.update("worker1", later, counted);
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        settle(&mut monitor, later);
        assert_eq!(monitor.error(), Some(BillingError::Unreachable));
        assert_eq!(
            monitor.sample().unwrap().buckets,
            first,
            "a failure keeps the last billing"
        );
        assert_eq!(monitor.sample().unwrap().fetched, start);

        let latest = start + REFRESH_INTERVAL * 2;
        monitor.update("worker1", latest, answer(Ok(Vec::new())));
        settle(&mut monitor, latest);
        assert_eq!(monitor.error(), None);
        assert!(monitor.sample().unwrap().buckets.is_empty());
    }

    #[test]
    fn a_stalled_refresh_times_out_and_is_cancelled() {
        let mut monitor = BillingMonitor::default();
        let (observed, cancelled) = mpsc::channel();
        let start = Instant::now();
        monitor.update("worker1", start, || {
            move |cancel: &Cancellation| {
                for _ in 0..1000 {
                    if cancel.is_cancelled() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = observed.send(cancel.is_cancelled());
                Err(BillingError::Unreachable)
            }
        });
        monitor.poll(start + REFRESH_TIMEOUT / 2);
        assert!(monitor.refreshing());
        monitor.poll(start + REFRESH_TIMEOUT);
        assert!(!monitor.refreshing());
        assert_eq!(monitor.error(), Some(BillingError::TimedOut));
        assert_eq!(cancelled.recv_timeout(Duration::from_secs(10)), Ok(true));

        let (observed, cancelled) = mpsc::channel();
        let mut dropped = BillingMonitor::default();
        dropped.update("worker1", start, || {
            move |cancel: &Cancellation| {
                while !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = observed.send(());
                Ok(Vec::new())
            }
        });
        drop(dropped);
        assert_eq!(cancelled.recv_timeout(Duration::from_secs(10)), Ok(()));
    }

    #[test]
    fn follow_refreshes_only_a_bound_worker_and_forgets_it_when_unbound() {
        let fetch: Fetch = |settings, pod, _, _| {
            assert!(settings.ends_with("cloud/settings.json"));
            assert_eq!(pod, "worker1");
            Ok(vec![bucket("2024-07-12T19:00:00Z", BucketSize::Hour, 0.25)])
        };
        let notified = Arc::new(AtomicUsize::new(0));
        let notify = {
            let notified = notified.clone();
            move || {
                notified.fetch_add(1, Ordering::Relaxed);
            }
        };
        let root = Path::new("/synthetic/cloud");
        let mut monitor = BillingMonitor::default();
        for operation in [json!({"state":"prepared"}), json!({"state":"requested"})] {
            monitor.follow(Some(&deployment(&operation)), Some(root), fetch, &notify);
            assert!(!monitor.refreshing());
        }
        let bound = deployment(&json!({"state":"bound","worker_id":"worker1"}));
        monitor.follow(Some(&bound), None, fetch, &notify);
        assert!(!monitor.refreshing(), "no settings, no refresh");
        monitor.follow(Some(&bound), Some(root), fetch, &notify);
        settle(&mut monitor, Instant::now());
        assert_eq!(notified.load(Ordering::Relaxed), 1);
        assert_eq!(monitor.sample().unwrap().buckets.len(), 1);

        monitor.record("worker2", Err(BillingError::Rejected), Instant::now());
        assert!(monitor.sample().is_none(), "another worker's billing is never reused");
        monitor.record("worker1", Ok(Vec::new()), Instant::now());
        let terminated = deployment(&json!({"state":"terminated","worker_id":"worker1"}));
        monitor.follow(Some(&terminated), Some(root), fetch, &notify);
        assert!(monitor.sample().is_none() && monitor.error().is_none());
        monitor.record("worker1", Ok(Vec::new()), Instant::now());
        monitor.follow(None, Some(root), fetch, &notify);
        assert!(monitor.sample().is_none());
    }

    #[test]
    fn fetch_reports_missing_settings_and_credentials_before_any_request() {
        let root = tempfile::tempdir().unwrap();
        let settings = root.path().join("settings.json");
        let cancel = Cancellation::default();
        let now = SystemTime::now();
        assert_eq!(fetch(&settings, "worker1", now, &cancel), Err(BillingError::Settings));
        std::fs::write(
            &settings,
            json!({
                "runpod_key_file":root.path().join("missing-key"),"ssh_identity_file":root.path().join("identity"),
                "docker_config":root.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(fetch(&settings, "worker1", now, &cancel), Err(BillingError::Credential));
    }

    #[test]
    fn day_and_hour_billing_meet_at_the_start_of_the_utc_day() {
        let now = at("2024-07-12T19:14:40.144Z");
        let today = start_of_day(now).unwrap();
        assert_eq!(today, at("2024-07-12T00:00:00Z"));
        assert_eq!(start_of_day(today), Some(today));
        let days = vec![
            bucket("2024-07-11T00:00:00Z", BucketSize::Day, 12.0),
            bucket("2024-07-12T00:00:00Z", BucketSize::Day, 3.0),
        ];
        let hours = vec![
            bucket("2024-07-11T23:00:00Z", BucketSize::Hour, 0.5),
            bucket("2024-07-12T00:00:00Z", BucketSize::Hour, 0.69),
            bucket("2024-07-12T02:00:00+02:00", BucketSize::Hour, 0.69),
        ];
        let combined = combine(days, hours, today);
        let kept: Vec<_> = combined.iter().map(|bucket| bucket.amount).collect();
        assert_eq!(kept, [12.0, 0.69, 0.69]);
    }

    #[test]
    fn provider_failures_become_short_reasons() {
        for (error, reason) in [
            (CloudError::Unauthorized, "RunPod rejected the API key"),
            (CloudError::Transport, "RunPod is unreachable"),
            (
                CloudError::Http(503, horizon_cloud::Reason::default()),
                "RunPod returned HTTP 503",
            ),
            (
                CloudError::Rejected(horizon_cloud::Reason::default()),
                "RunPod rejected the billing request",
            ),
            (CloudError::InvalidResponse, "RunPod returned invalid billing"),
            (CloudError::Cancelled, "billing could not be read"),
        ] {
            assert_eq!(BillingError::from(error).to_string(), reason);
        }
    }

    #[test]
    fn the_explanation_names_coverage_freshness_and_the_last_failure() {
        let fetched = Instant::now();
        let mut monitor = BillingMonitor::default();
        monitor.record("worker1", Ok(Vec::new()), fetched);
        let total = TotalCost {
            billed: 3.37,
            estimated: 0.83,
            billed_through: Some(at("2024-07-12T19:00:00Z")),
        };
        let text = monitor.explanation(&total, fetched + Duration::from_secs(130));
        assert!(text.starts_with(COVERAGE));
        assert!(text.contains("Network volume storage is not included"));
        assert!(text.ends_with("\nBilled through 2024-07-12 19:00 UTC.\nRefreshed 2m 10s ago."));
        monitor.record("worker1", Err(BillingError::Unreachable), fetched);
        let text = monitor.explanation(
            &TotalCost {
                billed_through: None,
                ..total
            },
            fetched,
        );
        assert!(text.ends_with("\nRefreshed 0m 00s ago.\nLast refresh failed: RunPod is unreachable."));
    }
}
