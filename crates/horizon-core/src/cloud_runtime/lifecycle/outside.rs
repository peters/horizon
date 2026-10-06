//! Stops that Horizon did not make: a worker that stopped itself, as its idle stop
//! does, or that was stopped through the provider account. Horizon learns of such a
//! stop when its connection to a ready worker fails, and then asks the provider.
use super::{ReconciledDeployment, reconcile_locked};
use crate::cloud_runtime::{
    CreateState, Error, Result, Stage,
    deployment::hetzner::idle::{IdleCheck, Report, read},
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, Worker,
    provider::{Description, IdleStop, StoppedCost},
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Prints the idle record only where the watcher keeps one. An older watcher writes
/// no record there, and it would start a second watcher if it were given `--report`.
const REPORT_COMMAND: &str = "[ ! -e /run/horizon-worker/idle.json ] || exec horizon-worker-idle --report";
/// How often the worker's watcher rewrites its record, so how old a record can be.
const WORKER_POLL: Duration = Duration::from_secs(60);
/// A sample older than this does not tell why a worker stopped.
const EVIDENCE_AGE: Duration = Duration::from_mins(10);
/// How often a provider check is tried, a second apart, while another operation
/// holds the cloud. Longer than the provider check it may wait for: a cancelled check
/// can still finish its settle and inspect requests, each bounded by the provider's
/// request timeout.
const BUSY_ATTEMPTS: u64 = 3 * horizon_cloud::runpod::REQUEST_TIMEOUT.as_secs();

/// The idle record of a running worker, and when Horizon read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleSample {
    pub idle: Duration,
    pub limit: Duration,
    pub read_at: Instant,
}

/// Why a worker stopped without an operation of this Horizon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopCause {
    /// Its idle stop: shortly before, its idle record had reached, or was about to
    /// reach, the idle period `limit`.
    Idle { limit: Duration },
    /// Horizon cannot tell. An agent on the worker or the provider account may have
    /// stopped it.
    Unknown,
}

impl StopCause {
    /// The cause of a stop found at `now`, from the newest idle `sample`. The sample
    /// explains the stop only when it is recent and the worker would have been idle
    /// for its whole period by `now`, allowing for the age of the record it read.
    #[must_use]
    pub fn of(sample: Option<&IdleSample>, now: Instant) -> Self {
        sample
            .filter(|sample| {
                let age = now.saturating_duration_since(sample.read_at);
                age <= EVIDENCE_AGE && sample.idle + age + WORKER_POLL >= sample.limit
            })
            .map_or(Self::Unknown, |sample| Self::Idle { limit: sample.limit })
    }
}

/// Whether a stop that Horizon did not make can explain a failed connection to the
/// cloud `state` records: a bound worker that Horizon does not stop, replace or
/// delete itself, on a provider whose stop keeps the worker. Only such a worker can
/// stop without Horizon; a Hetzner stop is Horizon's own release of the server.
#[must_use]
pub fn may_stop_outside(state: &Deployment) -> bool {
    Description::of(&state.profile).stopped == StoppedCost::WorkerKept
        && matches!(state.operation, CreateState::Bound { .. })
        && !matches!(state.stage, Stage::Stopping | Stage::Replace | Stage::Deleted)
}

/// Asks the provider, as Check provider does, about the worker of a cloud whose
/// connection failed. A stop the provider confirms is recorded as an ordinary
/// stopped cloud, so only Resume starts the worker again. The check waits while
/// another operation, such as another check of the same worker, holds the cloud.
///
/// Returns `None`, without asking the provider, when [`may_stop_outside`] refuses
/// the record.
/// # Errors
/// Reports a provider check that failed, or a cloud that another operation holds.
pub fn check_lost_worker(
    root: &Path,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<Option<ReconciledDeployment>> {
    check_with(root, cancel, Duration::from_secs(1), |store| {
        reconcile_locked(store, settings, None, cancel)
    })
}

/// As [`check_lost_worker`], with only the saved record of a confirmed stop.
/// # Errors
/// As [`check_lost_worker`].
pub fn worker_stopped(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<Option<Deployment>> {
    Ok(check_lost_worker(root, settings, cancel)?
        .filter(ReconciledDeployment::confirmed_stopped)
        .map(|reconciled| reconciled.state))
}

fn check_with(
    root: &Path,
    cancel: &Cancellation,
    pause: Duration,
    check: impl Fn(&Store) -> Result<ReconciledDeployment>,
) -> Result<Option<ReconciledDeployment>> {
    for attempt in 0..BUSY_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(pause);
        }
        // A cancelled check never takes the cloud, so it cannot hold it from the
        // check or operation that cancelled it.
        cancel.check()?;
        let store = match Store::lock(root) {
            Err(Error::Busy) => continue,
            other => other?,
        };
        if !store.load()?.as_ref().is_some_and(may_stop_outside) {
            return Ok(None);
        }
        return check(&store).map(Some);
    }
    Err(Error::Busy)
}

/// Reads the idle record of a ready cloud whose worker stops itself
/// (`provider::IdleStop::Worker`). Horizon only reads the record; it never stops
/// such a worker. A worker that keeps no record, as an older image, is `NotWatched`.
pub(super) fn sample(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<IdleCheck> {
    sample_with(root, |worker| read(worker, settings, root, cancel, REPORT_COMMAND))
}

fn sample_with(root: &Path, read: impl FnOnce(&Worker) -> Result<String>) -> Result<IdleCheck> {
    let Some(worker) = Store::lock(root)?.load()?.as_ref().and_then(sampled) else {
        return Ok(IdleCheck::NotWatched);
    };
    let output = read(&worker)?;
    if output.trim().is_empty() {
        return Ok(IdleCheck::NotWatched);
    }
    let report = Report::parse(&output)?;
    Ok(IdleCheck::Active {
        idle: Duration::from_secs(report.idle_seconds),
        limit: Duration::from_secs(report.idle_stop_seconds),
    })
}

/// The worker to sample: a running one with an idle period that stops itself.
fn sampled(state: &Deployment) -> Option<Worker> {
    (state.profile.idle_stop_minutes.is_some()
        && Description::of(&state.profile).idle_stop == IdleStop::Worker
        && matches!(state.operation, CreateState::Bound { .. })
        && state.stage == Stage::Ready
        && !state.stop_requested)
        .then(|| state.worker.clone())
        .flatten()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_cloud::runpod::recovery::{Outcome, Reconciliation};

    fn save(root: &Path, recorded: &str, provider: &str) -> Deployment {
        let profile = serde_json::json!({"provider":provider,"image":"registry.example/worker","cpu":2,
            "memory_gb":4,"idle_stop_minutes":30});
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"idle-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":recorded,"operation":{"state":"bound","worker_id":"worker1"},
            "spec":null,"sessions":[],"source_ready":true,
            "worker":{"id":"worker1","name":"idle-cloud","imageName":"registry.example/worker",
                "desiredStatus":"RUNNING","publicIp":"192.0.2.10","portMappings":{"22":40022}}
        }))
        .unwrap();
        Store::lock(root).unwrap().save(&state).unwrap();
        state
    }

    /// What the provider check reports for the saved record: the worker `status`,
    /// recorded as stopped when it is.
    fn reported(status: &'static str) -> impl Fn(&Store) -> Result<ReconciledDeployment> {
        move |store| {
            let mut state = store.load()?.unwrap();
            let worker: Worker = serde_json::from_value(serde_json::json!({
                "id":"worker1","name":"idle-cloud","imageName":"registry.example/worker","desiredStatus":status
            }))
            .unwrap();
            if status == "EXITED" {
                state.stage = Stage::Stopped;
                state.stop_requested = true;
            }
            state.worker = Some(worker.clone());
            store.save(&state)?;
            Ok(ReconciledDeployment {
                state,
                report: Reconciliation {
                    operation_id: "idle-cloud".into(),
                    outcome: Outcome::Inactive {
                        worker_id: "worker1".into(),
                    },
                    worker: Some(worker),
                },
            })
        }
    }

    #[test]
    fn a_recent_idle_record_near_its_period_explains_a_stop() {
        let start = Instant::now();
        let sample = |idle_minutes| IdleSample {
            idle: Duration::from_mins(idle_minutes),
            limit: Duration::from_mins(30),
            read_at: start,
        };
        let idle = StopCause::Idle {
            limit: Duration::from_mins(30),
        };
        let after = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(StopCause::of(Some(&sample(29)), after(30)), idle);
        // The record can be a whole watcher poll older than the read.
        assert_eq!(StopCause::of(Some(&sample(28)), after(60)), idle);
        assert_eq!(StopCause::of(Some(&sample(30)), start), idle);
        assert_eq!(
            StopCause::of(Some(&sample(5)), after(120)),
            StopCause::Unknown,
            "a busy worker was stopped by something else"
        );
        assert_eq!(
            StopCause::of(Some(&sample(29)), after(11 * 60)),
            StopCause::Unknown,
            "an old sample proves nothing"
        );
        assert_eq!(StopCause::of(None, start), StopCause::Unknown);
    }

    /// As [`worker_stopped`], with the provider check given.
    fn stopped_with(
        root: &Path,
        cancel: &Cancellation,
        check: impl Fn(&Store) -> Result<ReconciledDeployment>,
    ) -> Result<Option<Deployment>> {
        Ok(check_with(root, cancel, Duration::ZERO, check)?
            .filter(ReconciledDeployment::confirmed_stopped)
            .map(|reconciled| reconciled.state))
    }

    #[test]
    fn only_a_stop_the_provider_confirms_is_reported_and_recorded() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        save(temp.path(), "Ready", "runpod");
        let running = check_with(temp.path(), &cancel, Duration::ZERO, reported("RUNNING"))
            .unwrap()
            .unwrap();
        assert!(!running.confirmed_stopped(), "a running worker keeps the failure");
        assert!(
            stopped_with(temp.path(), &cancel, reported("RUNNING"))
                .unwrap()
                .is_none()
        );
        let exited = stopped_with(temp.path(), &cancel, reported("EXITED")).unwrap().unwrap();
        assert_eq!(exited.stage, Stage::Stopped);
        let saved = Store::lock(temp.path()).unwrap().load().unwrap().unwrap();
        assert!(
            saved.stop_requested && saved.stage == Stage::Stopped,
            "only Resume starts it"
        );
    }

    #[test]
    fn a_stop_horizon_makes_itself_or_a_foreign_record_is_not_checked() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let unasked = |_: &Store| -> Result<ReconciledDeployment> { panic!("the provider is not asked") };
        for stage in ["Stopping", "Replace", "Deleted"] {
            save(temp.path(), stage, "runpod");
            assert!(
                stopped_with(temp.path(), &cancel, unasked).unwrap().is_none(),
                "{stage}"
            );
        }
        let mut requested = save(temp.path(), "Provision", "runpod");
        requested.operation = CreateState::Requested;
        Store::lock(temp.path()).unwrap().save(&requested).unwrap();
        assert!(stopped_with(temp.path(), &cancel, unasked).unwrap().is_none());
        // A Hetzner stop releases the server; only Horizon makes it.
        save(temp.path(), "Ready", "hetzner");
        assert!(stopped_with(temp.path(), &cancel, unasked).unwrap().is_none());
    }

    #[test]
    fn a_held_cloud_is_checked_again_and_then_reported_busy() {
        let temp = tempfile::tempdir().unwrap();
        save(temp.path(), "Ready", "runpod");
        let held = Store::lock(temp.path()).unwrap();
        let result = stopped_with(temp.path(), &Cancellation::default(), reported("EXITED"));
        assert!(matches!(result, Err(Error::Busy)), "{result:?}");
        drop(held);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let held = Store::lock(temp.path()).unwrap();
        let result = stopped_with(temp.path(), &cancelled, reported("EXITED"));
        assert!(matches!(result, Err(Error::Provider(_))), "{result:?}");
        drop(held);
        // Even with the cloud free, a cancelled check neither takes it nor asks the provider.
        let unasked = |_: &Store| -> Result<ReconciledDeployment> { panic!("the provider is not asked") };
        assert!(matches!(
            stopped_with(temp.path(), &cancelled, unasked),
            Err(Error::Provider(_))
        ));
        assert!(Store::lock(temp.path()).is_ok(), "the cloud stays free");
    }

    #[test]
    fn a_self_stopping_worker_is_sampled_and_an_older_watcher_is_not() {
        let temp = tempfile::tempdir().unwrap();
        save(temp.path(), "Ready", "runpod");
        let record = || Ok(r#"{"idle_seconds":1700,"idle_stop_seconds":1800}"#.to_owned());
        assert_eq!(
            sample_with(temp.path(), |_| record()).unwrap(),
            IdleCheck::Active {
                idle: Duration::from_secs(1700),
                limit: Duration::from_mins(30),
            },
            "Horizon only reads it: the worker stops itself"
        );
        assert_eq!(
            sample_with(temp.path(), |_| Ok(String::new())).unwrap(),
            IdleCheck::NotWatched,
            "an older watcher keeps no record"
        );
        assert!(sample_with(temp.path(), |_| Ok("{}".into())).is_err());
        let unread = |_: &Worker| -> Result<String> { panic!("the worker is not read") };
        for (stage, provider) in [("Stopped", "runpod"), ("Ready", "hetzner")] {
            save(temp.path(), stage, provider);
            assert_eq!(sample_with(temp.path(), unread).unwrap(), IdleCheck::NotWatched);
        }
    }
}
