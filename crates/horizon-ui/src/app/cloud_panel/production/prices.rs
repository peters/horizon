//! Provider prices and stock for the New cloud dialog, fetched in the background and
//! refreshed while the dialog stays open. Only providers Horizon can deploy to are asked.
use super::machine_size::Size;
use horizon_core::cloud_runtime::{
    Cancellation,
    prices::{self, Preferences, PriceList, Profile, SizeAvailability},
    settings::Settings,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

pub(super) mod hetzner;

/// Prices and stock older than this are fetched again while the dialog is open.
pub(super) const FRESH: Duration = Duration::from_secs(15);
/// A catalog older than this can no longer start a cloud: prices and stock may have
/// changed too much since.
pub(super) const START_LIMIT: Duration = Duration::from_secs(60 * 60);
/// How long agents' requests get the same failed price fetch before one asks again.
const RETRY_FAILED: Duration = Duration::from_secs(30);

pub(super) struct Fetched<T> {
    pub value: T,
    pub at: Instant,
}

/// A CPU size together with the container disk that limits which flavors offer it.
type SizeKey = (u16, u16, u16, bool);
/// Answers are timestamped when the provider answers, not when a frame collects them.
type Job<T> = Receiver<Result<Fetched<T>, String>>;

#[derive(Default)]
pub(super) struct State {
    pub list: Option<Fetched<(PriceList, Preferences)>>,
    pub list_error: Option<String>,
    /// When the price fetch behind `list_error` failed.
    pub list_failed_at: Option<Instant>,
    list_job: Option<Job<(PriceList, Preferences)>>,
    /// Manual refresh invalidates current offers without removing their presentation.
    refreshed_after: Option<Instant>,
    sizes: HashMap<SizeKey, Result<Fetched<SizeAvailability>, String>>,
    size_jobs: HashMap<SizeKey, Job<SizeAvailability>>,
    size_failed_at: HashMap<SizeKey, Instant>,
    /// Every data center's region as people say it, kept from the latest list so cards
    /// can name where a worker landed without work on every frame.
    regions: HashMap<String, String>,
    /// Hetzner's catalog, fetched beside the list when this machine has a binding.
    pub hetzner: hetzner::State,
    /// A fetch found no `RunPod` API key. Kept through refreshes and expired errors,
    /// and cleared only by a list `RunPod` answered, so a retry never offers `RunPod`
    /// on a machine set up for Hetzner alone.
    runpod_missing: bool,
}

impl State {
    /// Starts background fetches for anything missing or stale about `profile`.
    pub fn request(&mut self, root: &Path, profile: &Profile, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach the provider.
        if cfg!(test) {
            return;
        }
        self.recheck_runpod();
        if self.list.as_ref().is_none_or(|list| !self.current(list.at)) {
            self.fetch_list(root, ctx);
        }
        // Hetzner is offered beside RunPod for CPU profiles when this machine has a binding.
        if !profile.gpu {
            self.hetzner.request(root, ctx);
        }
        let key = key(profile);
        let current = self
            .sizes
            .get(&key)
            .is_some_and(|size| size.as_ref().is_ok_and(|size| self.current(size.at)));
        if !profile.gpu
            && !current
            && !self.size_jobs.contains_key(&key)
            && !matches!(self.sizes.get(&key), Some(Err(_)))
        {
            let profile = profile.clone();
            let job = spawn(root, ctx, move |settings, cancel| {
                prices::size_availability(settings, &profile, cancel)
            });
            self.size_jobs.insert(key, job);
        }
        // Fetches repaint when they finish; an idle dialog still has to wake when its prices go stale.
        if let Some(wait) = self.until_stale(profile) {
            ctx.request_repaint_after(wait);
        }
    }

    /// Fetches the price list once when there is none, for naming the region of the data
    /// center a cloud landed in on its card. The New cloud dialog keeps it fresh.
    pub fn request_regions(&mut self, root: &Path, ctx: &egui::Context) {
        if cfg!(test) {
            return;
        }
        if self.list.is_none() {
            self.fetch_list(root, ctx);
        }
    }

    /// Fetches the price list when there is none or it is older than [`FRESH`], for
    /// answering agents' offer requests with current prices.
    pub fn request_fresh_list(&mut self, root: &Path, ctx: &egui::Context) {
        if cfg!(test) {
            return;
        }
        if self.list.as_ref().is_none_or(|list| !self.current(list.at)) {
            self.fetch_list(root, ctx);
        }
        self.hetzner.request(root, ctx);
    }

    /// The failed price fetch agents' requests report, the same one for every request
    /// that waited on it. Once it is older than [`RETRY_FAILED`] it is forgotten, so the
    /// next request asks the provider again.
    pub fn recent_list_error(&mut self) -> Option<&str> {
        self.recheck_runpod();
        self.list_error.as_deref()
    }

    /// The price list while it is current, never an older one.
    pub fn fresh_list(&self) -> Option<&Fetched<(PriceList, Preferences)>> {
        self.list.as_ref().filter(|list| self.current(list.at))
    }

    fn current(&self, at: Instant) -> bool {
        !stale(at) && self.refreshed_after.is_none_or(|refresh| at >= refresh)
    }

    /// Whether the catalog shown is older than [`START_LIMIT`], so it cannot start a cloud.
    pub fn too_old(&self) -> bool {
        self.list.as_ref().is_some_and(|list| list.at.elapsed() >= START_LIMIT)
    }

    /// The region of `data_center` as people say it, once a price list has been fetched.
    pub fn region_of(&self, data_center: &str) -> Option<&str> {
        self.regions.get(data_center).map(String::as_str)
    }

    fn accept(&mut self, fetched: Fetched<(PriceList, Preferences)>) {
        self.regions = fetched
            .value
            .0
            .regions
            .iter()
            .map(|(center, region)| (center.clone(), region_name(region)))
            .collect();
        self.list = Some(fetched);
    }

    fn fetch_list(&mut self, root: &Path, ctx: &egui::Context) {
        if self.list_job.is_none() && self.list_error.is_none() {
            self.list_job = Some(spawn(root, ctx, |settings, cancel| {
                prices::price_list(settings, cancel)
            }));
        }
    }

    /// Retry failed catalog requests after a pause, including settings reads that
    /// found no key. Keep the last known binding state until the provider answers.
    pub(super) fn recheck_runpod(&mut self) {
        if self.list_failed_at.is_some_and(|at| at.elapsed() >= RETRY_FAILED) {
            self.list_error = None;
            self.list_failed_at = None;
        }
        self.size_failed_at.retain(|key, at| {
            if at.elapsed() >= RETRY_FAILED {
                if matches!(self.sizes.get(key), Some(Err(_))) {
                    self.sizes.remove(key);
                }
                false
            } else {
                true
            }
        });
    }

    /// Time until the prices or stock shown for `profile` go stale, unless a fetch is running.
    fn until_stale(&self, profile: &Profile) -> Option<Duration> {
        let list = self
            .list
            .as_ref()
            .filter(|_| self.list_job.is_none())
            .map(|list| list.at);
        let size = Some(key(profile))
            .filter(|key| !profile.gpu && !self.size_jobs.contains_key(key))
            .and_then(|key| self.sizes.get(&key)?.as_ref().ok())
            .map(|size| size.at);
        list.into_iter()
            .chain(size)
            .map(|at| FRESH.saturating_sub(at.elapsed()))
            // Failed requests wake the idle dialog after the retry pause.
            .chain(
                self.list_failed_at
                    .filter(|_| self.list_job.is_none())
                    .map(|at| RETRY_FAILED.saturating_sub(at.elapsed())),
            )
            .chain(
                self.size_failed_at
                    .get(&key(profile))
                    .map(|at| RETRY_FAILED.saturating_sub(at.elapsed())),
            )
            .min()
    }

    /// Collects finished fetches.
    pub fn poll(&mut self) {
        self.hetzner.poll();
        if let Some(result) = finished(&mut self.list_job) {
            match result {
                Ok(fetched) => {
                    self.accept(fetched);
                    self.list_error = None;
                    self.list_failed_at = None;
                    self.runpod_missing = false;
                }
                // The last good catalog stays on show with its age; only current
                // prices are ever treated as current.
                Err(error) => {
                    self.runpod_missing |= error == horizon_core::cloud_runtime::settings::RUNPOD_KEY_MISSING;
                    self.refreshed_after = Some(Instant::now());
                    self.list_error = Some(error);
                    self.list_failed_at = Some(Instant::now());
                }
            }
        }
        let keys: Vec<SizeKey> = self.size_jobs.keys().copied().collect();
        for key in keys {
            let mut job = self.size_jobs.remove(&key);
            match finished(&mut job) {
                Some(result) => {
                    if result.is_err() {
                        self.size_failed_at.insert(key, Instant::now());
                    } else {
                        self.size_failed_at.remove(&key);
                    }
                    self.sizes.insert(key, result);
                }
                None => {
                    if let Some(job) = job {
                        self.size_jobs.insert(key, job);
                    }
                }
            }
        }
    }

    /// Requests new prices while retaining the last display and any requests in flight.
    pub fn refresh(&mut self) {
        self.refreshed_after = Some(Instant::now());
        self.list_error = None;
        self.list_failed_at = None;
        self.sizes.retain(|_, result| result.is_ok());
        self.size_failed_at.clear();
    }

    /// Whether this machine can use `RunPod`: false once a fetch found no API key, as on
    /// a machine set up for Hetzner alone, until `RunPod` answers a later fetch.
    pub fn runpod_bound(&self) -> bool {
        !self.runpod_missing
    }

    /// Whether it is not known yet if this machine has a `RunPod` key: `RunPod` has not
    /// answered, and a fetch is running or failed for another reason, such as settings
    /// saved only since.
    pub fn runpod_unknown(&self) -> bool {
        !self.runpod_missing && self.list.is_none() && (self.list_job.is_some() || self.list_error.is_some())
    }

    /// Whether `RunPod` has neither answered nor failed yet: no fetch was asked for,
    /// or the first one is still running.
    pub fn runpod_pending(&self) -> bool {
        !self.runpod_missing && self.list.is_none() && self.list_error.is_none()
    }

    pub fn loading(&self) -> bool {
        self.list_job.is_some() || !self.size_jobs.is_empty()
    }

    /// Stock of `size` for `profile`: `None` while it is being checked, including when
    /// its last answer has expired, so old stock is never shown as current.
    pub fn size(&self, profile: &Profile, size: Size) -> Option<Result<&SizeAvailability, &str>> {
        let sized = Profile {
            cpu: size.0,
            memory_gb: size.1,
            ..profile.clone()
        };
        let key = key(&sized);
        if self.size_jobs.contains_key(&key) {
            return None;
        }
        match self.sizes.get(&key)? {
            Ok(fetched) if !self.current(fetched.at) => None,
            Ok(fetched) => Some(Ok(&fetched.value)),
            Err(error) => Some(Err(error.as_str())),
        }
    }

    /// Last known stock for stable UI choices while fresh stock is being fetched.
    /// Agent offers continue to use the current catalog rather than this display cache.
    pub fn displayed_size(&self, profile: &Profile, size: Size) -> Option<Result<&SizeAvailability, &str>> {
        self.size(profile, size).or_else(|| {
            let sized = Profile {
                cpu: size.0,
                memory_gb: size.1,
                ..profile.clone()
            };
            self.sizes
                .get(&key(&sized))?
                .as_ref()
                .ok()
                .map(|fetched| Ok(&fetched.value))
        })
    }
}

/// The provider's region as people say it, such as `NORTH_AMERICA` as North America.
pub(super) fn region_name(region: &str) -> String {
    if region.is_empty() {
        return "Other".to_owned();
    }
    region
        .split('_')
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let mut letters = lower.chars();
            letters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + letters.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn stale(at: Instant) -> bool {
    at.elapsed() >= FRESH
}

fn key(profile: &Profile) -> SizeKey {
    (
        profile.cpu,
        profile.memory_gb,
        profile.storage.container_gb,
        profile.storage.standard_tier(),
    )
}

fn spawn<T: Send + 'static>(
    root: &Path,
    ctx: &egui::Context,
    fetch: impl FnOnce(&Settings, &Cancellation) -> horizon_core::cloud_runtime::Result<T> + Send + 'static,
) -> Job<T> {
    let (tx, rx) = channel();
    let path = root.join("settings.json");
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = Settings::load(&path)
            .and_then(|settings| fetch(&settings, &Cancellation::default()))
            .map(|value| Fetched {
                value,
                at: Instant::now(),
            })
            .map_err(|error| error.to_string());
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

fn finished<T>(job: &mut Option<Job<T>>) -> Option<Result<Fetched<T>, String>> {
    let result = match job.as_ref()?.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => Err("Price check ended without a result".into()),
    };
    *job = None;
    Some(result)
}

/// Prices and exact-size stock as if the provider had just answered, for the dialog
/// tests, which never contact it.
#[cfg(test)]
impl State {
    /// As the dialog is while its first `RunPod` fetch runs.
    #[cfg(unix)]
    pub fn runpod_checking(&mut self) {
        let (sender, receiver) = channel();
        std::mem::forget(sender);
        self.list_job = Some(receiver);
    }

    /// As a fetch finds it on a machine set up for Hetzner alone.
    #[cfg(unix)]
    pub fn runpod_key_missing(&mut self) {
        self.runpod_missing = true;
        self.list_error = Some(horizon_core::cloud_runtime::settings::RUNPOD_KEY_MISSING.to_owned());
    }

    /// As `RunPod` answers the first fetch with nothing on offer, so a `RunPod` cloud
    /// can be submitted.
    #[cfg(unix)]
    pub fn runpod_answered(&mut self) {
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: Vec::new(),
            regions: std::collections::BTreeMap::new(),
            storage: prices::RUNPOD_STORAGE,
        };
        self.answered(list, Preferences::default(), Vec::new());
    }

    pub fn answered(&mut self, list: PriceList, preferences: Preferences, sizes: Vec<(Profile, SizeAvailability)>) {
        self.accept(Fetched {
            value: (list, preferences),
            at: Instant::now(),
        });
        for (profile, size) in sizes {
            self.sizes.insert(
                key(&profile),
                Ok(Fetched {
                    value: size,
                    at: Instant::now(),
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::prices::Availability;

    fn list() -> PriceList {
        PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: Vec::new(),
            regions: std::collections::BTreeMap::new(),
            storage: prices::RUNPOD_STORAGE,
        }
    }

    fn profile() -> Profile {
        horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 32\n",
        )
        .unwrap()
        .profiles["dev"]
        .clone()
    }

    fn now<T>(value: T) -> Fetched<T> {
        Fetched {
            value,
            at: Instant::now(),
        }
    }

    fn available() -> SizeAvailability {
        SizeAvailability {
            centers: vec![
                ("EU-RO-1".into(), Availability::High),
                ("US-MO-2".into(), Availability::Low),
            ],
        }
    }

    #[test]
    fn premium_stock_never_reuses_standard_stock_even_during_refresh() {
        let mut state = State::default();
        let standard = profile();
        let mut premium = standard.clone();
        premium.storage.volume_tier = serde_json::from_value(serde_json::json!("HIGH_PERFORMANCE")).unwrap();
        state.sizes.insert(key(&standard), Ok(now(available())));
        let size = (standard.cpu, standard.memory_gb);
        assert!(state.size(&standard, size).is_some());
        assert!(state.size(&premium, size).is_none());
        assert!(state.displayed_size(&premium, size).is_none());
        state.refresh();
        assert!(state.displayed_size(&standard, size).is_some());
        assert!(state.displayed_size(&premium, size).is_none());
    }

    #[test]
    fn regions_read_as_people_say_them_and_are_found_by_data_center() {
        assert_eq!(region_name("NORTH_AMERICA"), "North America");
        assert_eq!(region_name("EUROPE"), "Europe");
        assert_eq!(region_name(""), "Other");
        let mut state = State::default();
        assert_eq!(state.region_of("EU-RO-1"), None);
        let mut listed = list();
        // Regions cover data centers the machine no longer allows, which a worker may
        // have landed in before the setting changed.
        listed.regions = [("EU-RO-1", "EUROPE"), ("US-MO-2", "NORTH_AMERICA")]
            .into_iter()
            .map(|(center, region)| (center.to_owned(), region.to_owned()))
            .collect();
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Ok(now((listed, Preferences::default())))).unwrap();
        state.poll();
        assert_eq!(state.region_of("EU-RO-1"), Some("Europe"));
        assert_eq!(state.region_of("US-MO-2"), Some("North America"));
        assert_eq!(state.region_of("AP-JP-1"), None);
        state.refresh();
        assert_eq!(state.region_of("EU-RO-1"), Some("Europe"), "regions outlive a refresh");
    }

    #[test]
    fn runpod_is_unknown_until_it_answers_or_is_found_missing() {
        let mut state = State::default();
        assert!(!state.runpod_unknown(), "nothing asked yet, as in the dialog tests");
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        assert!(state.runpod_unknown(), "asked, no answer");
        // A failure for another reason, such as settings not saved yet, proves nothing.
        tx.send(Err("Cloud state I/O failed".into())).unwrap();
        state.poll();
        assert!(state.runpod_unknown() && state.runpod_bound());
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Ok(now((list(), Preferences::default())))).unwrap();
        state.poll();
        assert!(!state.runpod_unknown() && state.runpod_bound());
    }

    #[test]
    fn a_missing_runpod_key_is_remembered_until_runpod_answers() {
        let mut state = State::default();
        assert!(state.runpod_bound(), "until a fetch says otherwise");
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err(horizon_core::cloud_runtime::settings::RUNPOD_KEY_MISSING.into()))
            .unwrap();
        state.poll();
        assert!(!state.runpod_bound());
        // Neither a refresh nor an expired error offers RunPod again.
        state.refresh();
        assert!(!state.runpod_bound());
        // The dialog wakes to ask again, and asks once the pause has passed.
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err(horizon_core::cloud_runtime::settings::RUNPOD_KEY_MISSING.into()))
            .unwrap();
        state.poll();
        assert!(state.until_stale(&profile()).is_some_and(|wait| wait <= RETRY_FAILED));
        state.recheck_runpod();
        assert!(state.list_error.is_some(), "not before the pause");
        state.list_failed_at = Instant::now().checked_sub(RETRY_FAILED);
        state.recheck_runpod();
        assert!(
            state.list_error.is_none() && !state.runpod_bound(),
            "asked again, still unknown to be bound"
        );
        // Another failure says nothing about the key.
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err("The provider is unavailable".into())).unwrap();
        state.poll();
        assert!(!state.runpod_bound());
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Ok(now((list(), Preferences::default())))).unwrap();
        state.poll();
        assert!(state.runpod_bound(), "a key added since is used");
    }

    #[test]
    fn finished_fetches_stay_visible_during_refresh_and_failures_wait_for_one() {
        let mut state = State::default();
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        assert!(state.loading());
        state.poll();
        assert!(state.list.is_none() && state.loading());
        tx.send(Ok(now((list(), Preferences::default())))).unwrap();
        state.poll();
        assert!(!state.loading());
        assert_eq!(state.list.as_ref().unwrap().value.0.provider, "RunPod");
        state.refresh();
        assert!(state.list.is_some() && state.fresh_list().is_none());

        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err("Missing RunPod API key".into())).unwrap();
        state.poll();
        assert_eq!(state.list_error.as_deref(), Some("Missing RunPod API key"));
        drop(state.list_job.take());
        let (tx, rx) = channel::<Result<Fetched<(PriceList, Preferences)>, String>>();
        drop(tx);
        state.list_job = Some(rx);
        state.poll();
        assert_eq!(state.list_error.as_deref(), Some("Price check ended without a result"));
    }

    #[test]
    fn size_stock_is_looked_up_for_the_chosen_size() {
        let profile = profile();
        let mut state = State::default();
        let available = available();
        let small = Profile {
            memory_gb: 16,
            ..profile.clone()
        };
        state.sizes.insert(
            key(&small),
            Ok(Fetched {
                value: available.clone(),
                at: Instant::now(),
            }),
        );
        state.sizes.insert(key(&profile), Err("No stock data".into()));
        assert_eq!(state.size(&profile, (8, 16)), Some(Ok(&available)));
        assert_eq!(state.size(&profile, (8, 32)), Some(Err("No stock data")));
        assert_eq!(state.size(&profile, (4, 8)), None);
        state.refresh();
        assert_eq!(state.size(&profile, (8, 16)), None);
    }

    #[test]
    fn a_failed_refresh_keeps_old_prices_but_never_as_current_and_refresh_keeps_checks_in_flight() {
        let mut state = State {
            list: Some(Fetched {
                value: (list(), Preferences::default()),
                at: Instant::now(),
            }),
            ..State::default()
        };
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err("RunPod is unreachable".into())).unwrap();
        state.poll();
        assert!(state.list.is_some(), "the last good catalog stays on show");
        assert!(state.fresh_list().is_none());
        assert_eq!(state.list_error.as_deref(), Some("RunPod is unreachable"));

        let (list_tx, rx) = channel();
        state.list_job = Some(rx);
        let (size_tx, rx) = channel();
        state.size_jobs.insert(key(&profile()), rx);
        state.refresh();
        assert!(state.loading() && state.list_error.is_none() && !state.size_jobs.is_empty());
        list_tx.send(Ok(now((list(), Preferences::default())))).unwrap();
        size_tx.send(Ok(now(available()))).unwrap();
        state.poll();
        assert!(state.fresh_list().is_some());
        assert!(state.size(&profile(), (8, 32)).is_some());
    }

    #[test]
    fn manual_refresh_retains_display_but_invalidates_current_offers() {
        let mut state = State {
            list: Some(now((list(), Preferences::default()))),
            ..State::default()
        };
        let profile = profile();
        state.sizes.insert(key(&profile), Ok(now(available())));
        state.refresh();
        assert!(state.list.is_some(), "keep region and GPU controls in place");
        assert!(state.fresh_list().is_none(), "agents wait for fresh offers");
        assert!(state.size(&profile, (profile.cpu, profile.memory_gb)).is_none());
        assert_eq!(
            state.displayed_size(&profile, (profile.cpu, profile.memory_gb)),
            Some(Ok(&available()))
        );
        assert!(!state.runpod_unknown(), "a refresh does not disable provider selection");
    }

    #[test]
    fn failed_catalog_refresh_wakes_and_retries_after_backoff() {
        let mut state = State::default();
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Err("Provider temporarily unavailable".into())).unwrap();
        state.poll();
        let wait = state.until_stale(&profile()).unwrap();
        assert!(wait <= RETRY_FAILED && wait > RETRY_FAILED / 2);
        state.recheck_runpod();
        assert!(state.list_error.is_some(), "do not retry every frame");
        state.list_failed_at = Instant::now().checked_sub(RETRY_FAILED);
        state.recheck_runpod();
        assert!(state.list_error.is_none() && state.list_failed_at.is_none());
        assert!(state.runpod_bound(), "a transient error does not remove the binding");
    }

    #[test]
    fn fresh_catalog_replaces_removed_gpu_offers() {
        let mut offered = list();
        offered.gpus.push(prices::GpuPrice {
            id: "removed".into(),
            name: "Removed GPU".into(),
            memory_gb: 24,
            hourly: 0.5,
        });
        let mut state = State::default();
        state.accept(Fetched {
            value: (offered, Preferences::default()),
            at: Instant::now().checked_sub(FRESH).unwrap(),
        });
        assert!(state.fresh_list().is_none(), "agents must not receive expired offers");
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Ok(now((list(), Preferences::default())))).unwrap();
        state.poll();
        assert!(state.fresh_list().unwrap().value.0.gpus.is_empty());
    }

    #[test]
    fn an_idle_dialog_wakes_when_its_prices_go_stale() {
        let cpu = profile();
        let gpu = Profile {
            gpu: true,
            ..cpu.clone()
        };
        let mut state = State::default();
        assert_eq!(state.until_stale(&cpu), None);
        state.sizes.insert(
            key(&cpu),
            Ok(Fetched {
                value: available(),
                at: Instant::now(),
            }),
        );
        let wait = state.until_stale(&cpu).unwrap();
        assert!(wait <= FRESH && wait > Duration::from_secs(14));
        assert_eq!(state.until_stale(&gpu), None);

        state.list = Some(Fetched {
            value: (list(), Preferences::default()),
            at: Instant::now(),
        });
        assert!(state.until_stale(&gpu).is_some());
        let (_tx, rx) = channel();
        state.list_job = Some(rx);
        assert_eq!(state.until_stale(&gpu), None);
    }

    #[test]
    fn answers_collected_late_keep_the_time_the_provider_answered() {
        let answered = Instant::now();
        let mut state = State::default();
        let (tx, rx) = channel();
        state.list_job = Some(rx);
        tx.send(Ok(Fetched {
            value: (list(), Preferences::default()),
            at: answered,
        }))
        .unwrap();
        let (tx, rx) = channel();
        state.size_jobs.insert(key(&profile()), rx);
        tx.send(Ok(Fetched {
            value: available(),
            at: answered,
        }))
        .unwrap();
        // A closed dialog collects answers later; they must not look newer than they are.
        std::thread::sleep(Duration::from_millis(5));
        state.poll();
        assert_eq!(state.list.as_ref().map(|list| list.at), Some(answered));
        let size = state.sizes.get(&key(&profile())).unwrap().as_ref().unwrap();
        assert_eq!(size.at, answered);
    }

    #[test]
    fn expired_or_rechecking_stock_reads_as_checking() {
        let profile = profile();
        let mut state = State::default();
        state.sizes.insert(key(&profile), Ok(now(available())));
        assert_eq!(state.size(&profile, (8, 32)), Some(Ok(&available())));
        let (_tx, rx) = channel();
        state.size_jobs.insert(key(&profile), rx);
        assert_eq!(state.size(&profile, (8, 32)), None);
        state.size_jobs.clear();
        // A freshly booted runner may not be able to express an expired instant.
        if let Some(expired) = Instant::now().checked_sub(FRESH) {
            state.sizes.insert(
                key(&profile),
                Ok(Fetched {
                    value: available(),
                    at: expired,
                }),
            );
            assert_eq!(state.size(&profile, (8, 32)), None);
        }
    }

    #[test]
    fn updating_covers_catalog_and_size_requests_in_either_completion_order() {
        for catalog_first in [true, false] {
            let mut state = State::default();
            let (list_tx, list_rx) = channel();
            let (size_tx, size_rx) = channel();
            state.list_job = Some(list_rx);
            state.size_jobs.insert(key(&profile()), size_rx);
            assert!(state.loading());
            if catalog_first {
                list_tx.send(Ok(now((list(), Preferences::default())))).unwrap();
            } else {
                size_tx.send(Ok(now(available()))).unwrap();
            }
            state.poll();
            assert!(state.loading(), "the remaining request still updates displayed stock");
            if catalog_first {
                size_tx.send(Ok(now(available()))).unwrap();
            } else {
                list_tx.send(Ok(now((list(), Preferences::default())))).unwrap();
            }
            state.poll();
            assert!(!state.loading());
        }
    }

    #[test]
    fn failed_size_stock_retries_after_a_bounded_pause_and_recovers() {
        let mut state = State::default();
        let profile = profile();
        let (tx, rx) = channel();
        state.size_jobs.insert(key(&profile), rx);
        tx.send(Err("Temporary stock failure".into())).unwrap();
        state.poll();
        state.recheck_runpod();
        assert_eq!(state.size(&profile, (8, 32)), Some(Err("Temporary stock failure")));
        assert!(
            state
                .until_stale(&profile)
                .is_some_and(|wait| !wait.is_zero() && wait <= RETRY_FAILED)
        );
        if let Some(expired) = Instant::now().checked_sub(RETRY_FAILED) {
            state.size_failed_at.insert(key(&profile), expired);
            state.recheck_runpod();
            assert!(state.size(&profile, (8, 32)).is_none());
            assert!(!state.sizes.contains_key(&key(&profile)), "the request path may retry");
            let (tx, rx) = channel();
            state.size_jobs.insert(key(&profile), rx);
            tx.send(Ok(now(available()))).unwrap();
            state.poll();
            assert_eq!(state.size(&profile, (8, 32)), Some(Ok(&available())));
            assert!(state.size_failed_at.is_empty());
        }
    }
}
