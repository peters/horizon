//! Keeps this Horizon's prices on its ready workers, so agents there can rank cloud
//! offers without the provider account. While any cloud is ready, prices refresh every
//! 15 minutes and each ready worker gets every fresh list once, with Hetzner's catalog
//! when this machine has a Hetzner binding.
use super::{HorizonApp, Runtime, prices::Fetched};
use horizon_core::cloud_runtime::{
    Cancellation, Stage,
    offer_publication::{self, HetznerSnapshot, Published, Snapshot, VERSION},
    prices::{Preferences, PriceList},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A worker that did not take the prices is asked again after this long.
const RETRY: Duration = Duration::from_mins(5);
/// Clouds are checked for workers due prices at most this often while nothing is sent.
const CHECK_EVERY: Duration = Duration::from_secs(5);

/// A worker without the current Hetzner catalog is asked again after this long. Older
/// worker images refuse it every time, so this is longer than [`RETRY`].
const HETZNER_RETRY: Duration = Duration::from_mins(15);

/// Sends the price list, when given, and then Hetzner's catalog, when there is one, to
/// the worker of a cloud. An error means the price list did not arrive.
type Publisher = Arc<
    dyn Fn(&Path, &str, Option<&Snapshot>, Option<&HetznerSnapshot>) -> Result<(Published, Hetzner), String>
        + Send
        + Sync,
>;

/// What happened to Hetzner's catalog in a send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hetzner {
    /// Nothing to send, or the price list did not arrive first.
    NotSent,
    Delivered,
    /// Refused or unreachable; asked again after [`HETZNER_RETRY`]. Worker images
    /// without Hetzner support refuse it this way too.
    Failed,
}

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

fn publish_over_ssh(
    root: &Path,
    cloud: &str,
    snapshot: Option<&Snapshot>,
    hetzner: Option<&HetznerSnapshot>,
) -> Result<(Published, Hetzner), String> {
    // UI tests resolve the developer's real Horizon home; they must never reach a worker.
    if cfg!(test) {
        return Ok((Published::NotReady, Hetzner::NotSent));
    }
    let cancel = Cancellation::default();
    let published = match snapshot {
        Some(snapshot) => {
            offer_publication::publish(root, cloud, snapshot, &cancel).map_err(|error| error.to_string())?
        }
        // Only the catalog is due; the worker has the current price list.
        None => Published::Sent,
    };
    let Some(hetzner) = hetzner.filter(|_| published == Published::Sent) else {
        return Ok((published, Hetzner::NotSent));
    };
    let outcome = match offer_publication::publish_hetzner(root, cloud, hetzner, &cancel) {
        Ok(Published::Sent) => Hetzner::Delivered,
        Ok(Published::NotReady) => Hetzner::Failed,
        Err(error) => {
            tracing::debug!(%error, "could not send Hetzner prices to a cloud worker");
            Hetzner::Failed
        }
    };
    Ok((published, outcome))
}

#[derive(Clone, Debug, PartialEq)]
struct Delivery {
    worker: String,
    /// The price fetch the worker has; `None` until one arrives.
    observed: Option<Instant>,
    /// No attempt before this, after a failed one.
    retry_at: Option<Instant>,
    /// The Hetzner catalog fetch the worker has; `None` until one arrives.
    hetzner: Option<Instant>,
    /// No Hetzner attempt before this, after a failed one.
    hetzner_retry_at: Option<Instant>,
}

struct Job {
    cloud: String,
    worker: String,
    /// The price fetch being sent; `None` when only the catalog is.
    observed: Option<Instant>,
    /// The Hetzner catalog fetch being sent, if any.
    hetzner: Option<Instant>,
    receiver: Receiver<Result<(Published, Hetzner), String>>,
}

impl State {
    /// Collects a finished send; the next frame then checks for more due workers.
    fn poll(&mut self, now: Instant) {
        let Some(job) = &self.job else { return };
        let outcome = match job.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            // A send that ended without an answer failed; it is retried like any other.
            Err(TryRecvError::Disconnected) => Err("the price send ended without an answer".to_owned()),
        };
        self.next_check = None;
        let previous = self
            .delivered
            .get(&job.cloud)
            .filter(|previous| previous.worker == job.worker)
            .cloned();
        let mut delivery = previous.clone().unwrap_or(Delivery {
            worker: job.worker.clone(),
            observed: None,
            retry_at: None,
            hetzner: None,
            hetzner_retry_at: None,
        });
        let hetzner = match &outcome {
            Ok((Published::Sent, hetzner)) => {
                if let Some(observed) = job.observed {
                    delivery.observed = Some(observed);
                    delivery.retry_at = None;
                }
                *hetzner
            }
            Ok((Published::NotReady, _)) | Err(_) => {
                if let Err(error) = &outcome {
                    tracing::debug!(%error, "could not send prices to a cloud worker");
                }
                if job.observed.is_some() {
                    delivery.retry_at = Some(now + RETRY);
                } else {
                    delivery.hetzner_retry_at = Some(now + HETZNER_RETRY);
                }
                Hetzner::NotSent
            }
        };
        match hetzner {
            Hetzner::Delivered => {
                delivery.hetzner = job.hetzner;
                delivery.hetzner_retry_at = None;
            }
            Hetzner::Failed => delivery.hetzner_retry_at = Some(now + HETZNER_RETRY),
            Hetzner::NotSent => {}
        }
        self.delivered.insert(job.cloud.clone(), delivery);
        self.job = None;
    }

    /// Starts sending `fetched`, with Hetzner's catalog when there is one, to the first
    /// ready worker that lacks the list; otherwise sends only the catalog to the first
    /// worker with the current list that lacks it.
    fn step(
        &mut self,
        root: &Path,
        ready: &[(String, String)],
        fetched: &Fetched<(PriceList, Preferences)>,
        hetzner: Option<(HetznerSnapshot, Instant)>,
        now: Instant,
        ctx: &egui::Context,
    ) {
        if self.job.is_some() {
            return;
        }
        let hetzner_at = hetzner.as_ref().map(|(_, at)| *at);
        let (target, snapshot) = if let Some(target) = due(&self.delivered, ready, fetched.at, now).cloned() {
            let (list, preferences) = &fetched.value;
            let snapshot = Snapshot {
                version: VERSION,
                observed_at_millis: observed_at_millis(fetched.at),
                list: list.clone(),
                preferences: preferences.clone(),
            };
            (target, Some(snapshot))
        } else if let Some(at) = hetzner_at
            && let Some(target) = due_hetzner(&self.delivered, ready, fetched.at, at, now).cloned()
        {
            (target, None)
        } else {
            return;
        };
        let (cloud, worker) = target;
        let observed = snapshot.as_ref().map(|_| fetched.at);
        let job = Job {
            receiver: start(
                Arc::clone(&self.publisher),
                root.to_owned(),
                cloud.clone(),
                (snapshot, hetzner.map(|(catalog, _)| catalog)),
                ctx.clone(),
            ),
            cloud,
            worker,
            observed,
            hetzner: hetzner_at,
        };
        self.job = Some(job);
    }
}

/// The next ready cloud whose worker has the current price list but lacks the Hetzner
/// catalog observed at `hetzner`, once any failed attempt's retry is due.
fn due_hetzner<'a>(
    delivered: &HashMap<String, Delivery>,
    ready: &'a [(String, String)],
    observed: Instant,
    hetzner: Instant,
    now: Instant,
) -> Option<&'a (String, String)> {
    ready.iter().find(|(cloud, worker)| {
        delivered.get(cloud).is_some_and(|delivery| {
            delivery.worker == *worker
                && delivery.observed == Some(observed)
                && delivery.hetzner != Some(hetzner)
                && delivery.hetzner_retry_at.is_none_or(|at| now >= at)
        })
    })
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
        // Hetzner's catalog travels with the list, so a running fetch is waited for.
        if prices.hetzner.pending() {
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }
        let hetzner = prices
            .hetzner
            .fresh()
            .and_then(|fetched| Some((fetched.value.clone()?, fetched.at)))
            .map(|(catalog, at)| {
                let snapshot = HetznerSnapshot {
                    version: VERSION,
                    observed_at_millis: observed_at_millis(at),
                    catalog,
                };
                (snapshot, at)
            });
        // Wake for the next refresh or the earliest recorded retry, whichever comes first.
        let refresh = super::prices::FRESH.saturating_sub(fetched.at.elapsed());
        let retry = publication
            .delivered
            .values()
            .flat_map(|delivery| [delivery.retry_at, delivery.hetzner_retry_at])
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now));
        ctx.request_repaint_after(retry.map_or(refresh, |retry| retry.min(refresh)));
        publication.step(&root, &ready, fetched, hetzner, now, ctx);
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
    (snapshot, hetzner): (Option<Snapshot>, Option<HetznerSnapshot>),
    ctx: egui::Context,
) -> Receiver<Result<(Published, Hetzner), String>> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let _ = sender.send(publisher(&root, &cloud, snapshot.as_ref(), hetzner.as_ref()));
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
            hetzner: None,
            hetzner_retry_at: None,
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
            hetzner: None,
            hetzner_retry_at: None,
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

    type Sent = (String, Option<Snapshot>, Option<HetznerSnapshot>);

    /// A state whose sends are recorded, answering the Hetzner part of each with the
    /// next of `outcomes`.
    fn recording(outcomes: Vec<Hetzner>) -> (State, Receiver<Sent>) {
        let (sent, received) = channel::<Sent>();
        let sent = std::sync::Mutex::new(sent);
        let outcomes = std::sync::Mutex::new(outcomes);
        let state = State {
            publisher: Arc::new(
                move |_: &Path, cloud: &str, snapshot: Option<&Snapshot>, hetzner: Option<&HetznerSnapshot>| {
                    sent.lock()
                        .unwrap()
                        .send((cloud.to_owned(), snapshot.cloned(), hetzner.cloned()))
                        .unwrap();
                    let outcome = if hetzner.is_some() {
                        outcomes.lock().unwrap().remove(0)
                    } else {
                        Hetzner::NotSent
                    };
                    Ok((Published::Sent, outcome))
                },
            ),
            ..State::default()
        };
        (state, received)
    }

    fn priced() -> Fetched<(PriceList, Preferences)> {
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
        Fetched {
            value: (list, preferences),
            at: Instant::now(),
        }
    }

    fn catalog(observed_at_millis: u64) -> HetznerSnapshot {
        HetznerSnapshot {
            version: VERSION,
            observed_at_millis,
            catalog: serde_json::from_value(serde_json::json!({
                "offers": [], "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {}, "ipv4_hour_eur": {}, "regions": {},
            }))
            .unwrap(),
        }
    }

    /// Collects the running send as of `now`.
    fn finish(state: &mut State, now: Instant) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.job.is_some() {
            assert!(Instant::now() < deadline, "the send finishes");
            state.poll(now);
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_ready_worker_receives_the_current_prices_and_is_marked_delivered() {
        let (mut state, received) = recording(vec![Hetzner::Delivered]);
        let fetched = priced();
        let ready = vec![("a".to_owned(), "w1".to_owned())];
        let ctx = egui::Context::default();
        let observed = Instant::now();
        let now = Instant::now();
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(1), observed)),
            now,
            &ctx,
        );
        let (cloud, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            sent_hetzner,
            Some(catalog(1)),
            "Hetzner's catalog travels with the list"
        );
        assert_eq!(cloud, "a");
        let snapshot = snapshot.unwrap();
        let (list, preferences) = fetched.value.clone();
        assert_eq!(
            (snapshot.version, snapshot.list, snapshot.preferences),
            (VERSION, list, preferences)
        );
        assert!(snapshot.observed_at_millis > 0);
        finish(&mut state, now);
        assert_eq!(state.delivered["a"].observed, Some(fetched.at));
        assert_eq!(state.delivered["a"].hetzner, Some(observed));
        assert!(state.next_check.is_none(), "the next frame checks for more due workers");
        // The worker has this list and catalog now, so nothing more is sent.
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(1), observed)),
            now,
            &ctx,
        );
        assert!(state.job.is_none());
    }

    #[test]
    fn a_newer_catalog_goes_alone_and_a_refused_one_waits_for_its_retry() {
        let (mut state, received) = recording(vec![Hetzner::Delivered, Hetzner::Delivered, Hetzner::Failed]);
        let fetched = priced();
        let ready = vec![("a".to_owned(), "w1".to_owned())];
        let ctx = egui::Context::default();
        let now = Instant::now();
        let first = Instant::now();
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(1), first)),
            now,
            &ctx,
        );
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        finish(&mut state, now);
        // A newer catalog goes to the worker alone, without the list it already has.
        let second = Instant::now();
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(2), second)),
            now,
            &ctx,
        );
        let (_, snapshot, sent_hetzner) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(snapshot.is_none());
        assert_eq!(sent_hetzner, Some(catalog(2)));
        finish(&mut state, now);
        assert_eq!(state.delivered["a"].hetzner, Some(second));
        // A catalog the worker refuses, as older images do, is asked again only after
        // its retry, while the list stays delivered.
        let third = Instant::now();
        let refused_at = Instant::now();
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(3), third)),
            refused_at,
            &ctx,
        );
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        finish(&mut state, refused_at);
        let delivery = &state.delivered["a"];
        assert_eq!(delivery.hetzner, Some(second), "a refused catalog is not recorded");
        assert_eq!(delivery.hetzner_retry_at, Some(refused_at + HETZNER_RETRY));
        assert_eq!(delivery.observed, Some(fetched.at));
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(3), third)),
            refused_at,
            &ctx,
        );
        assert!(state.job.is_none(), "no retry before it is due");
        state.step(
            Path::new("/unused"),
            &ready,
            &fetched,
            Some((catalog(3), third)),
            refused_at + HETZNER_RETRY,
            &ctx,
        );
        let (_, snapshot, _) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(snapshot.is_none(), "the retry sends only the catalog");
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
                hetzner: None,
                hetzner_retry_at: None,
            },
        );
        let (sender, receiver) = channel();
        state.job = Some(Job {
            cloud: "a".into(),
            worker: "w1".into(),
            observed: Some(now),
            hetzner: None,
            receiver,
        });
        sender.send(Err("unreachable".into())).unwrap();
        state.poll(now);
        // A send that ends without an answer is a failure too, never a stuck job.
        let expected = state.delivered["a"].clone();
        let (sender, receiver) = channel();
        drop(sender);
        state.job = Some(Job {
            cloud: "a".into(),
            worker: "w1".into(),
            observed: Some(now),
            hetzner: None,
            receiver,
        });
        state.poll(now);
        assert!(state.job.is_none());
        assert_eq!(state.delivered["a"], expected);
        assert!(state.job.is_none());
        assert_eq!(
            state.delivered["a"],
            Delivery {
                worker: "w1".into(),
                observed: Some(earlier),
                retry_at: Some(now + RETRY),
                hetzner: None,
                hetzner_retry_at: None,
            }
        );
    }
}
