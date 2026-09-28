//! Keeps this Horizon's prices on its ready workers, so agents there can rank cloud
//! offers without the provider account. While any cloud is ready, `RunPod` refreshes every
//! 15 seconds and Hetzner every 15 minutes. Each ready worker gets every fresh list once,
//! with Hetzner's catalog when this machine has a Hetzner binding. Without a `RunPod`
//! key, each worker is told once to drop the `RunPod` prices it may still hold.
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

/// Removes the `RunPod` prices the worker of a cloud holds.
type Clearer = Arc<dyn Fn(&Path, &str) -> Result<Published, String> + Send + Sync>;

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
    clearer: Clearer,
}

impl Default for State {
    fn default() -> Self {
        Self {
            delivered: HashMap::new(),
            job: None,
            next_check: None,
            publisher: Arc::new(publish_over_ssh),
            clearer: Arc::new(clear_over_ssh),
        }
    }
}

fn clear_over_ssh(root: &Path, cloud: &str) -> Result<Published, String> {
    // UI tests resolve the developer's real Horizon home; they must never reach a worker.
    if cfg!(test) {
        return Ok(Published::NotReady);
    }
    offer_publication::clear_runpod(root, cloud, &Cancellation::default()).map_err(|error| error.to_string())
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
    /// Whether the worker holds a Hetzner catalog, so the empty one that stands for no
    /// binding is kept current there; `None` when unknown, as after a restart.
    has_catalog: Option<bool>,
    /// No Hetzner attempt before this, after a failed one.
    hetzner_retry_at: Option<Instant>,
    /// Whether the worker may hold `RunPod` prices: `Some(true)` once this Horizon sent
    /// them, `Some(false)` once told to drop them, `None` when unknown, as after a restart.
    holds_runpod: Option<bool>,
}

struct Job {
    cloud: String,
    worker: String,
    /// The price fetch being sent; `None` when only the catalog is.
    observed: Option<Instant>,
    /// The Hetzner catalog fetch being sent, if any.
    hetzner: Option<Instant>,
    /// Whether the catalog being sent comes from a Hetzner binding, not the empty one.
    bound: bool,
    /// Whether this tells the worker to drop its `RunPod` prices instead of sending any.
    clear: bool,
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
            has_catalog: None,
            hetzner_retry_at: None,
            holds_runpod: None,
        });
        if job.clear {
            // Tried once per worker, whatever the outcome: older images refuse the
            // command every time, and their prices go stale on their own.
            if let Err(error) = &outcome {
                tracing::debug!(%error, "could not clear RunPod prices on a cloud worker");
            }
            delivery.holds_runpod = Some(false);
            self.delivered.insert(job.cloud.clone(), delivery);
            self.job = None;
            return;
        }
        let hetzner = match &outcome {
            Ok((Published::Sent, hetzner)) => {
                if let Some(observed) = job.observed {
                    delivery.observed = Some(observed);
                    delivery.retry_at = None;
                    delivery.holds_runpod = Some(true);
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
                delivery.has_catalog = Some(true);
                delivery.hetzner_retry_at = None;
            }
            // Clearing a worker that may never have had a catalog is tried once; older
            // images refuse it every time.
            Hetzner::Failed if !job.bound && delivery.has_catalog.is_none() => {
                delivery.has_catalog = Some(false);
            }
            Hetzner::Failed => delivery.hetzner_retry_at = Some(now + HETZNER_RETRY),
            Hetzner::NotSent => {}
        }
        self.delivered.insert(job.cloud.clone(), delivery);
        self.job = None;
    }

    /// Starts sending `fetched`, with Hetzner's catalog when there is one, to the first
    /// ready worker that lacks the list; otherwise sends only the catalog to the first
    /// worker with the current list that lacks it. With `clear_runpod`, as when this
    /// machine has no `RunPod` key, workers that may hold `RunPod` prices are first told
    /// to drop them.
    fn step(
        &mut self,
        root: &Path,
        ready: &[(String, String)],
        (fetched, clear_runpod): (Option<&Fetched<(PriceList, Preferences)>>, bool),
        hetzner: Option<(HetznerSnapshot, Instant)>,
        now: Instant,
        ctx: &egui::Context,
    ) {
        if self.job.is_some() {
            return;
        }
        if clear_runpod && let Some((cloud, worker)) = holding_runpod(&self.delivered, ready).cloned() {
            let (sender, receiver) = channel();
            let clearer = Arc::clone(&self.clearer);
            let (root, target, ctx) = (root.to_owned(), cloud.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = sender.send(clearer(&root, &target).map(|published| (published, Hetzner::NotSent)));
                ctx.request_repaint();
            });
            self.job = Some(Job {
                cloud,
                worker,
                observed: None,
                hetzner: None,
                bound: false,
                clear: true,
                receiver,
            });
            return;
        }
        let hetzner_at = hetzner.as_ref().map(|(_, at)| *at);
        // An empty catalog only clears offers a worker was given; others never need it.
        // A bound catalog goes to every worker, even with no offers after location
        // filtering. The empty one that stands for no binding only keeps workers that
        // hold a catalog current, so it never goes stale there.
        let bound = hetzner
            .as_ref()
            .is_some_and(|(snapshot, _)| snapshot.catalog != empty_catalog());
        let observed = fetched.map(|fetched| fetched.at);
        let (target, snapshot) = if let Some(fetched) = fetched
            && let Some(target) = due(&self.delivered, ready, fetched.at, now).cloned()
        {
            let (list, preferences) = &fetched.value;
            let snapshot = Snapshot {
                version: VERSION,
                observed_at_millis: observed_at_millis(fetched.at),
                list: list.clone(),
                preferences: preferences.clone(),
            };
            (target, Some(snapshot))
        } else if let Some(at) = hetzner_at
            && let Some(target) = due_hetzner(&self.delivered, ready, (observed, at, bound), now).cloned()
        {
            (target, None)
        } else {
            return;
        };
        let (cloud, worker) = target;
        let observed = snapshot.as_ref().and(observed);
        let had_catalog = self
            .delivered
            .get(&cloud)
            .filter(|delivery| delivery.worker == worker)
            .is_none_or(|delivery| delivery.has_catalog != Some(false));
        let hetzner = hetzner.filter(|_| bound || had_catalog);
        let hetzner_at = hetzner.as_ref().map(|(_, at)| *at);
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
            bound,
            clear: false,
        };
        self.job = Some(job);
    }
}

/// The next ready cloud whose worker may still hold `RunPod` prices: this Horizon sent
/// it some, or it is not known, as for a worker first seen since Horizon started.
fn holding_runpod<'a>(
    delivered: &HashMap<String, Delivery>,
    ready: &'a [(String, String)],
) -> Option<&'a (String, String)> {
    ready.iter().find(|(cloud, worker)| {
        delivered
            .get(cloud)
            .filter(|delivery| delivery.worker == *worker)
            .is_none_or(|delivery| delivery.holds_runpod != Some(false))
    })
}

/// The next ready cloud whose worker has the current price list but lacks the Hetzner
/// catalog observed at `hetzner`, once any failed attempt's retry is due. Without a
/// `RunPod` list (`observed` is `None`, as on a machine set up for Hetzner alone), the
/// catalog goes on its own, to workers that have had nothing yet as well.
fn due_hetzner<'a>(
    delivered: &HashMap<String, Delivery>,
    ready: &'a [(String, String)],
    (observed, hetzner, bound): (Option<Instant>, Instant, bool),
    now: Instant,
) -> Option<&'a (String, String)> {
    ready.iter().find(|(cloud, worker)| {
        let Some(delivery) = delivered.get(cloud).filter(|delivery| delivery.worker == *worker) else {
            return observed.is_none() && bound;
        };
        observed.is_none_or(|observed| delivery.observed == Some(observed))
            && delivery.hetzner != Some(hetzner)
            && (bound || delivery.has_catalog != Some(false))
            && delivery.hetzner_retry_at.is_none_or(|at| now >= at)
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
        // A RunPod key added since is found once the failed fetch's pause has passed.
        prices.recheck_runpod();
        // A failed fetch is asked again once agents' requests stop reporting it.
        if prices.recent_list_error().is_none() {
            prices.request_fresh_list(&root, ctx);
        }
        let fetched = prices.fresh_list();
        // A machine set up for Hetzner alone publishes its catalog without a RunPod list.
        if fetched.is_none() && prices.runpod_bound() {
            ctx.request_repaint_after(Duration::from_secs(30));
            return;
        }
        // Hetzner's catalog travels with the list, so a running fetch is waited for, but
        // only briefly: a slow Hetzner never holds back the list.
        if prices.hetzner.worth_waiting_for() {
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }
        // Without a Hetzner binding an empty catalog is sent, so a worker that had one
        // stops offering it rather than keeping it until it goes stale.
        let hetzner = prices
            .hetzner
            .fresh()
            .map(|fetched| (fetched.value.clone().unwrap_or_else(empty_catalog), fetched.at))
            .map(|(catalog, at)| {
                let snapshot = HetznerSnapshot {
                    version: VERSION,
                    observed_at_millis: observed_at_millis(at),
                    catalog,
                };
                (snapshot, at)
            });
        // Wake for the next refresh or the earliest recorded retry, whichever comes first.
        // Hetzner's catalog is refreshed on its own schedule, so wake for whichever
        // goes stale first; workers must never be left with an old catalog.
        let catalog_refresh = prices.hetzner.refresh_in();
        let refresh = fetched
            .map_or(Duration::MAX, |fetched| {
                super::prices::FRESH.saturating_sub(fetched.at.elapsed())
            })
            .min(catalog_refresh.unwrap_or(Duration::MAX));
        // Retries already due are handled by this step; only future ones need a wake.
        let retry = publication
            .delivered
            .values()
            .flat_map(|delivery| [delivery.retry_at, delivery.hetzner_retry_at])
            .flatten()
            .filter(|at| *at > now)
            .min()
            .map(|at| at.saturating_duration_since(now))
            .into_iter()
            .chain(prices.hetzner.retry_in().filter(|wait| !wait.is_zero()))
            .min();
        ctx.request_repaint_after(retry.map_or(refresh, |retry| retry.min(refresh)));
        let clear_runpod = fetched.is_none() && !prices.runpod_bound();
        publication.step(&root, &ready, (fetched, clear_runpod), hetzner, now, ctx);
    }
}

/// A catalog with no offers, sent when this machine has no Hetzner binding.
fn empty_catalog() -> horizon_core::cloud_runtime::prices::HetznerCatalog {
    horizon_core::cloud_runtime::prices::HetznerCatalog {
        offers: Vec::new(),
        volume_gb_month_eur: 0.0,
        ipv4_month_eur: std::collections::BTreeMap::new(),
        ipv4_hour_eur: std::collections::BTreeMap::new(),
        regions: std::collections::BTreeMap::new(),
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
mod tests;

#[cfg(test)]
mod clearing {
    #[test]
    fn a_removed_binding_sends_an_empty_catalog_that_workers_accept() {
        let empty = super::empty_catalog();
        assert!(empty.offers.is_empty());
        let snapshot = super::HetznerSnapshot {
            version: super::VERSION,
            observed_at_millis: 1,
            catalog: empty,
        };
        assert!(snapshot.validate().is_ok(), "workers accept the empty catalog");
    }
}
