//! Keeps this Horizon's prices on its ready workers, so agents there can rank cloud
//! offers without the provider account. While any cloud is ready, prices refresh every
//! 15 minutes and each ready worker gets every fresh list once.
use super::{HorizonApp, Runtime, prices::Fetched};
use horizon_core::cloud_runtime::{
    Cancellation, Stage,
    offer_publication::{self, Published, Snapshot, VERSION},
    prices::{Preferences, PriceList},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A worker that did not take the prices is asked again after this long.
const RETRY: Duration = Duration::from_mins(5);
/// Clouds are checked for workers due prices at most this often while nothing is sent.
const CHECK_EVERY: Duration = Duration::from_secs(5);

/// Sends a snapshot to the worker of a cloud.
type Publisher = Arc<dyn Fn(&Path, &str, &Snapshot) -> Result<Published, String> + Send + Sync>;

pub(super) struct State {
    /// Per cloud, the delivery its worker last had.
    delivered: HashMap<String, Delivery>,
    job: Option<Job>,
    /// No check for due workers before this, so idle frames do no work.
    next_check: Option<Instant>,
    publisher: Publisher,
}

impl Default for State {
    fn default() -> Self {
        Self {
            delivered: HashMap::new(),
            job: None,
            next_check: None,
            publisher: Arc::new(publish_over_ssh),
        }
    }
}

fn publish_over_ssh(root: &Path, cloud: &str, snapshot: &Snapshot) -> Result<Published, String> {
    // UI tests resolve the developer's real Horizon home; they must never reach a worker.
    if cfg!(test) {
        return Ok(Published::NotReady);
    }
    offer_publication::publish(root, cloud, snapshot, &Cancellation::default()).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, PartialEq)]
struct Delivery {
    worker: String,
    /// The price fetch the worker has; `None` until one arrives.
    observed: Option<Instant>,
    /// No attempt before this, after a failed one.
    retry_at: Option<Instant>,
}

struct Job {
    cloud: String,
    worker: String,
    observed: Instant,
    receiver: Receiver<Result<Published, String>>,
}

impl State {
    /// Collects a finished send; the next frame then checks for more due workers.
    fn poll(&mut self, now: Instant) {
        let Some(job) = &self.job else { return };
        let Ok(outcome) = job.receiver.try_recv() else {
            return;
        };
        self.next_check = None;
        let delivery = match outcome {
            Ok(Published::Sent) => Delivery {
                worker: job.worker.clone(),
                observed: Some(job.observed),
                retry_at: None,
            },
            Ok(Published::NotReady) | Err(_) => {
                if let Err(error) = &outcome {
                    tracing::debug!(%error, "could not send prices to a cloud worker");
                }
                let previous = self
                    .delivered
                    .get(&job.cloud)
                    .filter(|previous| previous.worker == job.worker);
                Delivery {
                    worker: job.worker.clone(),
                    observed: previous.and_then(|previous| previous.observed),
                    retry_at: Some(now + RETRY),
                }
            }
        };
        self.delivered.insert(job.cloud.clone(), delivery);
        self.job = None;
    }

    /// Starts sending `fetched` to the first ready worker that lacks it.
    fn step(
        &mut self,
        root: &Path,
        ready: &[(String, String)],
        fetched: &Fetched<(PriceList, Preferences)>,
        now: Instant,
        ctx: &egui::Context,
    ) {
        if self.job.is_some() {
            return;
        }
        let Some((cloud, worker)) = due(&self.delivered, ready, fetched.at, now).cloned() else {
            return;
        };
        let (list, preferences) = &fetched.value;
        let snapshot = Snapshot {
            version: VERSION,
            observed_at_millis: observed_at_millis(fetched.at),
            list: list.clone(),
            preferences: preferences.clone(),
        };
        let job = Job {
            receiver: start(
                Arc::clone(&self.publisher),
                root.to_owned(),
                cloud.clone(),
                snapshot,
                ctx.clone(),
            ),
            cloud,
            worker,
            observed: fetched.at,
        };
        self.job = Some(job);
    }
}

/// The next ready cloud whose worker lacks the prices observed at `observed`. Workers
/// not tried since they last took prices, or since they replaced another, come first;
/// failed ones follow once their retry is due, the longest waiting first, so unreachable
/// workers never hold up the rest.
fn due<'a>(
    delivered: &HashMap<String, Delivery>,
    ready: &'a [(String, String)],
    observed: Instant,
    now: Instant,
) -> Option<&'a (String, String)> {
    let untried = ready.iter().find(|(cloud, worker)| {
        delivered.get(cloud).is_none_or(|delivery| {
            delivery.worker != *worker || (delivery.observed != Some(observed) && delivery.retry_at.is_none())
        })
    });
    untried.or_else(|| {
        ready
            .iter()
            .filter_map(|entry| {
                let delivery = delivered.get(&entry.0)?;
                let retry_at = delivery.retry_at?;
                (delivery.worker == entry.1 && delivery.observed != Some(observed) && now >= retry_at)
                    .then_some((retry_at, entry))
            })
            .min_by_key(|(retry_at, _)| *retry_at)
            .map(|(_, entry)| entry)
    })
}

/// `(cloud, worker)` when the runtime's worker is ready for prices, by the same rule the
/// SSH transport applies, and no newer stage says otherwise.
fn ready_worker(runtime: &Runtime) -> Option<(String, String)> {
    let state = runtime.state.as_ref()?;
    let worker = state.worker.as_ref()?;
    (state.worker_ready() && runtime.stage.is_none_or(|stage| stage == Stage::Ready))
        .then(|| (state.cloud_id.clone(), worker.id.clone()))
}

impl HorizonApp {
    /// Sends fresh prices to ready workers, one worker at a time.
    pub(super) fn publish_cloud_offers(&mut self, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let now = Instant::now();
        let production = &mut self.cloud_prototype.production;
        let publication = &mut production.offer_publication;
        publication.poll(now);
        if publication.job.is_some() {
            return;
        }
        if let Some(at) = publication.next_check.filter(|at| now < *at) {
            // An earlier frame must not swallow the check: ask for one when it is due.
            ctx.request_repaint_after(at - now);
            return;
        }
        publication.next_check = Some(now + CHECK_EVERY);
        let mut ready: Vec<_> = production.runtimes.values().filter_map(ready_worker).collect();
        ready.sort();
        publication
            .delivered
            .retain(|cloud, _| ready.iter().any(|(id, _)| id == cloud));
        if ready.is_empty() {
            return;
        }
        let prices = &mut production.prices;
        prices.poll();
        // A failed fetch is asked again once agents' requests stop reporting it.
        if prices.recent_list_error().is_none() {
            prices.request_fresh_list(&root, ctx);
        }
        let Some(fetched) = prices.fresh_list() else {
            ctx.request_repaint_after(Duration::from_secs(30));
            return;
        };
        ctx.request_repaint_after(super::prices::FRESH.saturating_sub(fetched.at.elapsed()).min(RETRY));
        publication.step(&root, &ready, fetched, now, ctx);
    }
}

fn observed_at_millis(at: Instant) -> u64 {
    SystemTime::now()
        .checked_sub(at.elapsed())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| u64::try_from(time.as_millis()).unwrap_or(u64::MAX))
}

fn start(
    publisher: Publisher,
    root: PathBuf,
    cloud: String,
    snapshot: Snapshot,
    ctx: egui::Context,
) -> Receiver<Result<Published, String>> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let _ = sender.send(publisher(&root, &cloud, &snapshot));
        ctx.request_repaint();
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_ready_worker_gets_each_fresh_list_once_and_failures_wait() {
        let now = Instant::now();
        let first = now.checked_sub(Duration::from_mins(20)).unwrap();
        let ready = vec![("a".to_owned(), "w1".to_owned()), ("b".to_owned(), "w2".to_owned())];
        let mut delivered = HashMap::new();
        assert_eq!(due(&delivered, &ready, first, now), Some(&ready[0]));
        let sent = |worker: &str, observed| Delivery {
            worker: worker.into(),
            observed: Some(observed),
            retry_at: None,
        };
        delivered.insert("a".to_owned(), sent("w1", first));
        assert_eq!(due(&delivered, &ready, first, now), Some(&ready[1]));
        delivered.insert("b".to_owned(), sent("w2", first));
        assert_eq!(due(&delivered, &ready, first, now), None);
        // A fresh list goes to every worker again.
        assert_eq!(due(&delivered, &ready, now, now), Some(&ready[0]));
        // A worker that failed waits before it is asked again, unless it was replaced.
        delivered.insert(
            "a".to_owned(),
            Delivery {
                retry_at: Some(now + RETRY),
                ..sent("w1", first)
            },
        );
        delivered.insert("b".to_owned(), sent("w2", now));
        assert_eq!(due(&delivered, &ready, now, now), None);
        assert_eq!(due(&delivered, &ready, now, now + RETRY), Some(&ready[0]));
        let replaced = vec![("a".to_owned(), "w3".to_owned())];
        assert_eq!(due(&delivered, &replaced, now, now), Some(&replaced[0]));
        // A worker never tried goes before failed ones whose retry is due, and among
        // those the longest waiting goes first.
        let three = vec![
            ("a".to_owned(), "w1".to_owned()),
            ("b".to_owned(), "w2".to_owned()),
            ("c".to_owned(), "w4".to_owned()),
        ];
        let failed = |retry_at| Delivery {
            worker: String::new(),
            observed: None,
            retry_at: Some(retry_at),
        };
        let mut retries = HashMap::new();
        retries.insert(
            "a".to_owned(),
            Delivery {
                worker: "w1".into(),
                ..failed(now + RETRY)
            },
        );
        retries.insert(
            "b".to_owned(),
            Delivery {
                worker: "w2".into(),
                ..failed(now)
            },
        );
        assert_eq!(due(&retries, &three, now, now + RETRY), Some(&three[2]));
        retries.insert(
            "c".to_owned(),
            Delivery {
                worker: "w4".into(),
                ..failed(now + RETRY * 2)
            },
        );
        assert_eq!(due(&retries, &three, now, now + RETRY), Some(&three[1]));
    }

    #[test]
    fn a_ready_worker_receives_the_current_prices_and_is_marked_delivered() {
        let (sent, received) = channel();
        let sent = std::sync::Mutex::new(sent);
        let mut state = State {
            publisher: Arc::new(move |_: &Path, cloud: &str, snapshot: &Snapshot| {
                sent.lock().unwrap().send((cloud.to_owned(), snapshot.clone())).unwrap();
                Ok(Published::Sent)
            }),
            ..State::default()
        };
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: Vec::new(),
            regions: std::collections::BTreeMap::new(),
            storage: horizon_core::cloud_runtime::prices::RUNPOD_STORAGE,
        };
        let preferences = Preferences {
            cpu_flavors: vec!["cpu3c".into()],
            gpu_types: Vec::new(),
        };
        let fetched = Fetched {
            value: (list.clone(), preferences.clone()),
            at: Instant::now(),
        };
        let ready = vec![("a".to_owned(), "w1".to_owned())];
        let ctx = egui::Context::default();
        state.step(Path::new("/unused"), &ready, &fetched, Instant::now(), &ctx);
        let (cloud, snapshot) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cloud, "a");
        assert_eq!(
            (snapshot.version, snapshot.list, snapshot.preferences),
            (VERSION, list, preferences)
        );
        assert!(snapshot.observed_at_millis > 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.job.is_some() {
            assert!(Instant::now() < deadline, "the send finishes");
            state.poll(Instant::now());
            std::thread::yield_now();
        }
        assert_eq!(state.delivered["a"].observed, Some(fetched.at));
        assert!(state.next_check.is_none(), "the next frame checks for more due workers");
        // The worker has this list now, so nothing more is sent.
        state.step(Path::new("/unused"), &ready, &fetched, Instant::now(), &ctx);
        assert!(state.job.is_none());
    }

    #[test]
    fn a_failed_delivery_keeps_the_prices_the_worker_already_has() {
        let now = Instant::now();
        let earlier = now.checked_sub(Duration::from_mins(15)).unwrap();
        let mut state = State::default();
        state.delivered.insert(
            "a".to_owned(),
            Delivery {
                worker: "w1".into(),
                observed: Some(earlier),
                retry_at: None,
            },
        );
        let (sender, receiver) = channel();
        state.job = Some(Job {
            cloud: "a".into(),
            worker: "w1".into(),
            observed: now,
            receiver,
        });
        sender.send(Err("unreachable".into())).unwrap();
        state.poll(now);
        assert!(state.job.is_none());
        assert_eq!(
            state.delivered["a"],
            Delivery {
                worker: "w1".into(),
                observed: Some(earlier),
                retry_at: Some(now + RETRY),
            }
        );
    }
}
