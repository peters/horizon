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

/// Prices and stock older than this are fetched again while the dialog is open.
pub(super) const FRESH: Duration = Duration::from_mins(15);

pub(super) struct Fetched<T> {
    pub value: T,
    pub at: Instant,
}

/// A CPU size together with the container disk that limits which flavors offer it.
type SizeKey = (u16, u16, u16);
/// Answers are timestamped when the provider answers, not when a frame collects them.
type Job<T> = Receiver<Result<Fetched<T>, String>>;

#[derive(Default)]
pub(super) struct State {
    pub list: Option<Fetched<(PriceList, Preferences)>>,
    pub list_error: Option<String>,
    list_job: Option<Job<(PriceList, Preferences)>>,
    sizes: HashMap<SizeKey, Result<Fetched<SizeAvailability>, String>>,
    size_jobs: HashMap<SizeKey, Job<SizeAvailability>>,
}

impl State {
    /// Starts background fetches for anything missing or stale about `profile`.
    pub fn request(&mut self, root: &Path, profile: &Profile, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach the provider.
        if cfg!(test) {
            return;
        }
        if self.list_job.is_none() && self.list.as_ref().is_none_or(|list| stale(list.at)) && self.list_error.is_none()
        {
            self.list_job = Some(spawn(root, ctx, |settings, cancel| {
                prices::price_list(settings, cancel)
            }));
        }
        let key = key(profile);
        let current = self
            .sizes
            .get(&key)
            .is_some_and(|size| size.as_ref().is_ok_and(|size| !stale(size.at)));
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
            .min()
    }

    /// Collects finished fetches.
    pub fn poll(&mut self) {
        if let Some(result) = finished(&mut self.list_job) {
            match result {
                Ok(fetched) => {
                    self.list = Some(fetched);
                    self.list_error = None;
                }
                // Prices that could not be refreshed are no longer shown as current.
                Err(error) => {
                    self.list = None;
                    self.list_error = Some(error);
                }
            }
        }
        let keys: Vec<SizeKey> = self.size_jobs.keys().copied().collect();
        for key in keys {
            let mut job = self.size_jobs.remove(&key);
            match finished(&mut job) {
                Some(result) => {
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

    /// Forgets fetched prices, errors and checks in flight so the next frame asks again.
    pub fn refresh(&mut self) {
        self.list = None;
        self.list_error = None;
        self.list_job = None;
        self.sizes.clear();
        self.size_jobs.clear();
    }

    pub fn loading(&self) -> bool {
        self.list_job.is_some()
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
            Ok(fetched) if stale(fetched.at) => None,
            Ok(fetched) => Some(Ok(&fetched.value)),
            Err(error) => Some(Err(error.as_str())),
        }
    }
}

fn stale(at: Instant) -> bool {
    at.elapsed() >= FRESH
}

fn key(profile: &Profile) -> SizeKey {
    (profile.cpu, profile.memory_gb, profile.storage.container_gb)
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

/// Prices and exact-size stock as if the provider had just answered, for UI tests that
/// never contact it.
#[cfg(test)]
impl State {
    pub fn answered(&mut self, list: PriceList, preferences: Preferences, sizes: Vec<(Profile, SizeAvailability)>) {
        self.list = Some(Fetched {
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
    fn finished_fetches_are_kept_until_a_refresh_and_failures_wait_for_one() {
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
        assert!(state.list.is_none());

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
    fn a_failed_refresh_hides_old_prices_and_refresh_drops_checks_in_flight() {
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
        assert!(state.list.is_none());
        assert_eq!(state.list_error.as_deref(), Some("RunPod is unreachable"));

        let (list_tx, rx) = channel();
        state.list_job = Some(rx);
        let (size_tx, rx) = channel();
        state.size_jobs.insert(key(&profile()), rx);
        state.refresh();
        assert!(!state.loading() && state.list_error.is_none() && state.size_jobs.is_empty());
        assert!(list_tx.send(Ok(now((list(), Preferences::default())))).is_err());
        assert!(size_tx.send(Ok(now(available()))).is_err());
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
        assert!(wait <= FRESH && wait > Duration::from_mins(14));
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
        // A runner booted less than 15 minutes ago cannot express an expired instant.
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
}
