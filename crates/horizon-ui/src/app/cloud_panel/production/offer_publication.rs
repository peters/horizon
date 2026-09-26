//! Keeps this Horizon's prices on its ready workers, so agents there can rank cloud
//! offers without the provider account. While any cloud is ready, prices refresh every
//! 15 minutes and each ready worker gets every fresh list once.
use super::{HorizonApp, Runtime};
use horizon_core::cloud_runtime::{
    Cancellation, Stage,
    offer_publication::{self, Published, Snapshot, VERSION},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A worker that did not take the prices is asked again after this long.
const RETRY: Duration = Duration::from_mins(5);

#[derive(Default)]
pub(super) struct State {
    /// Per cloud, the delivery its worker last had.
    delivered: HashMap<String, Delivery>,
    job: Option<Job>,
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
    fn poll(&mut self, now: Instant) {
        let Some(job) = &self.job else { return };
        let Ok(outcome) = job.receiver.try_recv() else {
            return;
        };
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
}

/// The ready clouds whose worker lacks the prices observed at `observed`, first one first.
fn due<'a>(
    delivered: &HashMap<String, Delivery>,
    ready: &'a [(String, String)],
    observed: Instant,
    now: Instant,
) -> Option<&'a (String, String)> {
    ready.iter().find(|(cloud, worker)| {
        delivered.get(cloud).is_none_or(|delivery| {
            delivery.worker != *worker
                || (delivery.observed != Some(observed) && delivery.retry_at.is_none_or(|at| now >= at))
        })
    })
}

/// `(cloud, worker)` when the runtime's worker is ready for prices.
fn ready_worker(runtime: &Runtime) -> Option<(String, String)> {
    let state = runtime.state.as_ref()?;
    let worker = state.worker.as_ref()?;
    (state.stage == Stage::Ready && !state.stop_requested && worker.desired_status == "RUNNING")
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
        if publication.job.is_some() {
            return;
        }
        let Some((cloud, worker)) = due(&publication.delivered, &ready, fetched.at, now).cloned() else {
            return;
        };
        let (list, preferences) = &fetched.value;
        let snapshot = Snapshot {
            version: VERSION,
            observed_at_millis: observed_at_millis(fetched.at),
            list: list.clone(),
            preferences: preferences.clone(),
        };
        publication.job = Some(start(root, cloud, worker, fetched.at, snapshot, ctx.clone()));
    }
}

fn observed_at_millis(at: Instant) -> u64 {
    SystemTime::now()
        .checked_sub(at.elapsed())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| u64::try_from(time.as_millis()).unwrap_or(u64::MAX))
}

fn start(
    root: PathBuf,
    cloud: String,
    worker: String,
    observed: Instant,
    snapshot: Snapshot,
    ctx: egui::Context,
) -> Job {
    let (sender, receiver) = channel();
    let id = cloud.clone();
    std::thread::spawn(move || {
        let outcome = if cfg!(test) {
            Ok(Published::NotReady)
        } else {
            offer_publication::publish(&root, &id, &snapshot, &Cancellation::default())
                .map_err(|error| error.to_string())
        };
        let _ = sender.send(outcome);
        ctx.request_repaint();
    });
    Job {
        cloud,
        worker,
        observed,
        receiver,
    }
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
