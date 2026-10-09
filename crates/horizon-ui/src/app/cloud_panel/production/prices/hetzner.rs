//! Hetzner's catalog, fetched beside the `RunPod` price list when this machine has a
//! Hetzner binding, for agents' offer requests and the prices sent to workers.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished, spawn};
use horizon_core::cloud_runtime::prices::{self, HetznerCatalog, HetznerExclusions, freshness};
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime},
};

/// Longest wait for a running Hetzner fetch before prices go to workers without it.
const WAIT_FOR_FETCH: Duration = Duration::from_secs(20);
/// Hetzner's catalog keeps its own cadence; fast GPU stock polling is RunPod-specific.
const FRESH: Duration = Duration::from_mins(15);
/// A fetch's catalog with the configured server types and locations, or the reason it
/// failed for a machine with a Hetzner binding.
type Fetch = Result<(Option<HetznerCatalog>, Vec<String>, Vec<String>, HetznerExclusions), String>;

#[derive(Default)]
pub(in crate::app) struct State {
    /// The latest catalog; `None` inside when this machine has no Hetzner binding.
    fetched: Option<Fetched<Option<HetznerCatalog>>>,
    /// The server types this machine's settings try, in order, from the same fetch.
    server_types: Vec<String>,
    /// The locations this machine's settings allow, in the order deployment tries them.
    locations: Vec<String>,
    /// Server types and locations the settings left out of the latest catalog.
    exclusions: HetznerExclusions,
    /// Whether the last finished fetch found a Hetzner binding, kept while the catalog is
    /// refreshed and when fetching it failed.
    bound: bool,
    error: Option<String>,
    failed_at: Option<Instant>,
    /// A failure with a binding comes back as an `Err` inside, so the binding is known.
    job: Option<Job<Fetch>>,
    /// When the running fetch started.
    started: Option<Instant>,
    /// When the settings the last fetch read were saved.
    settings_saved: Option<SystemTime>,
    refreshed_after: Option<Instant>,
    /// Instant stored by test [`Self::answered`]. A descheduled runner must not age that
    /// fixture past [`FRESH`]. Backdating `fetched.at`, or [`Self::refresh`], uses the real rules.
    #[cfg(test)]
    fixture_answered_at: Option<Instant>,
}

impl State {
    /// Fetches the catalog when there is none or it is older than [`FRESH`]. A failed
    /// fetch is asked again once it is older than [`RETRY_FAILED`].
    pub(super) fn request(&mut self, root: &Path, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach Hetzner.
        if cfg!(test) {
            return;
        }
        let saved = std::fs::metadata(root.join("settings.json"))
            .and_then(|metadata| metadata.modified())
            .ok();
        self.forget_if_settings_changed(saved);
        if self.failed_at.is_some_and(|at| at.elapsed() >= RETRY_FAILED) {
            self.error = None;
            self.failed_at = None;
        }
        if self.job.is_none()
            && self.error.is_none()
            && self.fetched.as_ref().is_none_or(|fetched| {
                fetched.at.elapsed() >= FRESH || self.refreshed_after.is_some_and(|refresh| fetched.at < refresh)
            })
        {
            self.job = Some(spawn(root, ctx, |settings, cancel| {
                let (server_types, locations) = settings
                    .hetzner
                    .as_ref()
                    .map(|hetzner| (hetzner.server_types.clone(), hetzner.locations.clone()))
                    .unwrap_or_default();
                match prices::hetzner_catalog_report(settings, cancel) {
                    Ok(Some(report)) => Ok(Ok((Some(report.catalog), server_types, locations, report.exclusions))),
                    Ok(None) => Ok(Ok((None, server_types, locations, HetznerExclusions::default()))),
                    Err(error) if settings.hetzner.is_some() => Ok(Err(error.to_string())),
                    Err(error) => Err(error),
                }
            }));
            self.started = Some(Instant::now());
            self.settings_saved = saved;
        }
    }

    /// Forgets the catalog and a fetch still running: both were made with the credential that
    /// was just replaced, and the next request asks with the new one.
    pub(super) fn restart(&mut self) {
        self.job = None;
        self.started = None;
        self.fetched = None;
        self.exclusions = HetznerExclusions::default();
        self.error = None;
        self.failed_at = None;
    }

    /// Saved settings can add, change or remove the binding, so a catalog or failure from
    /// earlier settings is fetched again. The binding found is kept until then.
    fn forget_if_settings_changed(&mut self, saved: Option<SystemTime>) {
        if self.job.is_none() && saved != self.settings_saved {
            self.fetched = None;
            self.exclusions = HetznerExclusions::default();
            self.error = None;
            self.failed_at = None;
        }
    }

    pub(super) fn poll(&mut self) {
        let finished = finished(&mut self.job);
        if self.job.is_none() {
            self.started = None;
        }
        match finished {
            Some(Ok(Fetched {
                value: Ok((catalog, server_types, locations, exclusions)),
                at,
            })) => {
                self.bound = catalog.is_some();
                self.fetched = Some(Fetched { value: catalog, at });
                self.server_types = server_types;
                self.locations = locations;
                self.exclusions = exclusions;
                self.error = None;
                self.failed_at = None;
            }
            Some(Ok(Fetched { value: Err(error), .. })) => {
                self.bound = true;
                self.fail(error);
            }
            Some(Err(error)) => self.fail(error),
            None => {}
        }
    }

    /// A catalog that could not be refreshed is kept, but it is never offered as
    /// current: a refresh starts only once it has gone stale.
    fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.failed_at = Some(Instant::now());
    }

    /// Whether senders should wait for a running fetch: only for [`WAIT_FOR_FETCH`], so
    /// a slow Hetzner never holds back the `RunPod` prices.
    pub fn worth_waiting_for(&self) -> bool {
        self.job.is_some() && self.started.is_some_and(|started| started.elapsed() < WAIT_FOR_FETCH)
    }

    /// Until a failed fetch may be asked again, for waking an idle app.
    pub fn retry_in(&self) -> Option<Duration> {
        self.failed_at.map(|at| RETRY_FAILED.saturating_sub(at.elapsed()))
    }

    /// Whether this machine has a Hetzner binding, as the last finished fetch found,
    /// even while its catalog is refreshed or could not be fetched.
    pub fn bound(&self) -> bool {
        self.bound
    }

    /// The server types this machine's settings try, in order.
    #[cfg(test)]
    pub fn server_types(&self) -> &[String] {
        &self.server_types
    }

    /// The locations this machine's settings allow, in the order deployment tries them.
    #[cfg(test)]
    pub fn locations(&self) -> &[String] {
        &self.locations
    }

    /// Server types and locations the settings left out of the catalog on show.
    pub fn exclusions(&self) -> HetznerExclusions {
        self.exclusions
    }

    /// The reason the last fetch failed, while it is reported.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The current catalog, when this machine has a Hetzner binding.
    pub fn refresh(&mut self) {
        self.refreshed_after = Some(Instant::now());
        self.failed_at = None;
        self.error = None;
    }

    pub fn fresh(&self) -> Option<&Fetched<Option<HetznerCatalog>>> {
        self.fetched.as_ref().filter(|fetched| {
            fetched.at.elapsed() < FRESH && self.refreshed_after.is_none_or(|refresh| fetched.at >= refresh)
        })
    }

    /// The catalog while the dialog may compare with it: current, or gone stale while a
    /// background refresh for it runs. Failures are reported through [`Self::error`].
    pub fn comparable(&self) -> Option<&Fetched<Option<HetznerCatalog>>> {
        self.fetched
            .as_ref()
            .filter(|fetched| self.catalog_is_comparable(fetched))
    }

    fn catalog_is_comparable(&self, fetched: &Fetched<Option<HetznerCatalog>>) -> bool {
        #[cfg(test)]
        if self.fixture_answered_at.is_some_and(|anchor| {
            fetched.at >= anchor && self.refreshed_after.is_none_or(|refresh| fetched.at >= refresh)
        }) {
            return true;
        }
        freshness::comparable(
            Some(freshness::Answer::since(fetched.at, self.refreshed_after)),
            FRESH,
            freshness::Fetch::running(self.job.is_some()),
        )
    }

    /// Until a catalog whose refresh is running stops counting as current, for waking an
    /// idle dialog then.
    pub(super) fn grace_left(&self) -> Option<Duration> {
        self.fetched
            .as_ref()
            .filter(|_| self.job.is_some())
            .and_then(|fetched| freshness::grace_left(fetched.at.elapsed(), FRESH))
    }

    /// The last catalog fetched, however old, for showing choices while a refresh runs
    /// or after it failed. Decisions that need current prices use [`Self::fresh`].
    pub fn displayed(&self) -> Option<&Fetched<Option<HetznerCatalog>>> {
        self.fetched.as_ref()
    }

    /// Whether the catalog shown is older than the limit for starting a cloud.
    pub fn too_old(&self) -> bool {
        self.fetched
            .as_ref()
            .is_some_and(|fetched| fetched.at.elapsed() >= super::START_LIMIT)
    }

    /// Time until this provider's current catalog needs another fetch.
    pub fn refresh_in(&self) -> Option<Duration> {
        self.fresh().map(|fetched| FRESH.saturating_sub(fetched.at.elapsed()))
    }

    /// Hetzner's part of a `cloud_offers` answer: empty without a binding, the reason
    /// when the fetch failed, and offers in euros from a current catalog. `None` while its
    /// catalog is being fetched and more than [`ANSWER_MARGIN_MILLIS`] remain until
    /// `deadline_in_millis`; closer to the deadline Hetzner is reported as still being
    /// fetched, so the rest of the answer is not lost.
    pub fn sections(
        &self,
        requirements: &horizon_core::cloud_runtime::offers::Requirements,
        deadline_in_millis: i64,
    ) -> Option<Vec<serde_json::Value>> {
        if requirements.gpu {
            return Some(Vec::new());
        }
        if self.job.is_some() {
            if deadline_in_millis > ANSWER_MARGIN_MILLIS {
                return None;
            }
            return Some(vec![serde_json::json!({
                "provider": "Hetzner",
                "error": "cloud_offers_unavailable: Hetzner prices are still being fetched",
            })]);
        }
        if let Some(error) = &self.error {
            return Some(vec![
                serde_json::json!({"provider": "Hetzner", "error": format!("cloud_offers_unavailable: {error}")}),
            ]);
        }
        let Some(fetched) = self.fresh() else {
            // Nothing asked yet, as in tests, which never reach Hetzner.
            return Some(Vec::new());
        };
        let Some(catalog) = &fetched.value else {
            return Some(Vec::new());
        };
        let mut section = horizon_core::cloud_runtime::offers::hetzner_section(catalog, requirements);
        let observed = std::time::SystemTime::now()
            .checked_sub(fetched.at.elapsed())
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|at| u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
        section["observed_at_millis"] = serde_json::json!(observed);
        section["observed_seconds_ago"] = serde_json::json!(fetched.at.elapsed().as_secs());
        Some(vec![section])
    }
}

/// A catalog with the configured server types, for the dialog tests, which run on Unix
/// only.
#[cfg(all(test, unix))]
impl State {
    pub fn pending_fetch(&mut self) -> impl FnOnce(Option<HetznerCatalog>) + use<> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.job = Some(receiver);
        self.started = Some(Instant::now());
        move |catalog| {
            assert!(
                sender
                    .send(Ok(Fetched {
                        value: Ok((catalog, Vec::new(), Vec::new(), HetznerExclusions::default())),
                        at: Instant::now(),
                    }))
                    .is_ok()
            );
        }
    }

    /// As a refresh fails after a catalog was fetched: the catalog stays on show.
    pub fn refresh_failed(&mut self, error: &str) {
        self.bound = true;
        self.fail(error.to_owned());
    }

    pub fn answered_with_types(&mut self, catalog: Option<HetznerCatalog>, server_types: &[&str]) {
        self.answered(catalog);
        self.server_types = server_types.iter().map(|&name| name.to_owned()).collect();
    }

    /// As `answered_with_types`, with the allowed locations in the settings' order.
    pub fn answered_with_policy(&mut self, catalog: Option<HetznerCatalog>, server_types: &[&str], locations: &[&str]) {
        self.answered_with_types(catalog, server_types);
        self.locations = locations.iter().map(|&name| name.to_owned()).collect();
    }
}

/// A catalog as if Hetzner had just answered, for tests, which never contact it.
#[cfg(test)]
impl State {
    pub fn answered(&mut self, catalog: Option<HetznerCatalog>) {
        self.bound = catalog.is_some();
        let at = Instant::now();
        self.fixture_answered_at = Some(at);
        self.fetched = Some(Fetched { value: catalog, at });
        self.exclusions = HetznerExclusions::default();
    }

    /// As [`Self::answered`], with the types and locations the settings left out.
    pub fn answered_with_exclusions(&mut self, catalog: Option<HetznerCatalog>, exclusions: HetznerExclusions) {
        self.answered(catalog);
        self.exclusions = exclusions;
    }

    /// A fetch that failed for a machine with a Hetzner binding.
    pub fn failed(&mut self, error: &str) {
        self.bound = true;
        self.fetched = None;
        self.error = Some(error.to_owned());
        self.failed_at = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> HetznerCatalog {
        serde_json::from_value(serde_json::json!({
            "offers": [{"server_type": "cx43", "location": "hel1", "cores": 8, "memory_gb": 16.0, "disk_gb": 160,
                "dedicated": false, "hourly_eur": 0.0256, "monthly_eur": 15.99, "available": false, "recommended": false}],
            "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5}, "ipv4_hour_eur": {"hel1": 0.0008},
            "regions": {"hel1": "EUROPE"},
        }))
        .unwrap()
    }

    #[test]
    fn a_background_refresh_keeps_the_last_catalog_comparable_until_it_answers() {
        let mut state = State::default();
        assert!(state.comparable().is_none(), "nothing answered yet");
        // A freshly booted runner may not be able to express an earlier instant.
        let (Some(answered_at), Some(expired)) = (
            Instant::now().checked_sub(FRESH + Duration::from_secs(5)),
            Instant::now().checked_sub(FRESH + freshness::REFRESH_GRACE),
        ) else {
            return;
        };
        state.fetched = Some(Fetched {
            value: Some(catalog()),
            at: answered_at,
        });
        assert!(state.comparable().is_none(), "stale with no refresh running");
        let (_sender, receiver) = std::sync::mpsc::channel();
        state.job = Some(receiver);
        assert!(state.comparable().is_some(), "the refresh keeps the last catalog");
        assert!(state.fresh().is_none(), "agents still wait for a current catalog");
        assert!(
            state.grace_left().is_some_and(|left| left <= freshness::REFRESH_GRACE),
            "the dialog wakes when the grace ends"
        );
        state.fetched.as_mut().unwrap().at = expired;
        assert!(state.comparable().is_none(), "a refresh that takes too long");
        state.fetched.as_mut().unwrap().at = answered_at;
        state.refresh();
        assert!(state.comparable().is_none(), "a manual refresh waits for a new catalog");
    }

    #[test]
    fn catalog_keeps_its_cadence_when_runpod_stock_expires() {
        let mut state = State {
            fetched: Some(Fetched {
                value: Some(catalog()),
                at: Instant::now().checked_sub(super::super::FRESH).unwrap(),
            }),
            ..State::default()
        };
        assert!(state.fresh().is_some());
        assert!(state.refresh_in().is_some_and(|wait| wait > Duration::from_mins(14)));
        if let Some(expired) = Instant::now().checked_sub(FRESH) {
            state.fetched.as_mut().unwrap().at = expired;
            assert!(state.fresh().is_none());
            assert!(state.refresh_in().is_none());
        }
    }

    #[test]
    fn runpod_refresh_preserves_other_provider_choices_and_pending_fetch() {
        let mut prices = super::super::State::default();
        prices.hetzner.answered(Some(catalog()));
        prices.hetzner.server_types = vec!["cx43".into()];
        prices.hetzner.locations = vec!["hel1".into()];
        let (sender, receiver) = std::sync::mpsc::channel();
        prices.hetzner.job = Some(receiver);
        prices.refresh();
        assert!(prices.runpod_bound() && prices.hetzner.bound());
        assert!(prices.hetzner.fresh().is_none());
        assert!(prices.hetzner.displayed().is_some());
        assert_eq!(prices.hetzner.server_types(), ["cx43"]);
        assert_eq!(prices.hetzner.locations(), ["hel1"]);
        sender.send(Err("Synthetic failure".into())).unwrap();
        prices.poll();
        assert_eq!(prices.hetzner.error(), Some("Synthetic failure"));
        assert!(prices.hetzner.bound());
    }

    #[test]
    fn sections_follow_the_binding_the_fetch_and_its_failure() {
        let requirements = horizon_core::cloud_runtime::offers::Requirements::default();
        let mut state = State::default();
        assert_eq!(
            state.sections(&requirements, i64::MAX),
            Some(Vec::new()),
            "nothing asked yet"
        );
        state.answered(None);
        assert_eq!(
            state.sections(&requirements, i64::MAX),
            Some(Vec::new()),
            "no Hetzner binding"
        );
        state.answered(Some(catalog()));
        let sections = state.sections(&requirements, i64::MAX).unwrap();
        assert_eq!(
            (sections[0]["provider"].as_str(), sections[0]["currency"].as_str()),
            (Some("Hetzner"), Some("EUR"))
        );
        assert_eq!(sections[0]["offers"][0]["id"], "cx43");
        assert_eq!(sections[0]["observed_seconds_ago"], 0);
        assert!(
            sections[0]["observed_at_millis"].as_u64().is_some_and(|at| at > 0),
            "as workers report it"
        );
        state.failed("Missing Hetzner token");
        let failed = state.sections(&requirements, i64::MAX).unwrap();
        assert_eq!(failed[0]["error"], "cloud_offers_unavailable: Missing Hetzner token");
        // While a fetch runs, the answer waits for it.
        let (_sender, receiver) = std::sync::mpsc::channel();
        state.job = Some(receiver);
        assert_eq!(state.sections(&requirements, i64::MAX), None);
        let late = state.sections(&requirements, 1_000).unwrap();
        assert_eq!(
            late[0]["error"],
            "cloud_offers_unavailable: Hetzner prices are still being fetched"
        );
    }

    #[test]
    fn saving_settings_fetches_the_catalog_again() {
        let saved = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let mut state = State::default();
        state.answered(Some(catalog()));
        state.settings_saved = Some(saved);
        state.forget_if_settings_changed(Some(saved));
        assert!(state.fresh().is_some(), "unchanged settings keep the catalog");
        state.failed("Hetzner answered 503");
        state.forget_if_settings_changed(Some(saved + Duration::from_secs(1)));
        assert!(state.fresh().is_none() && state.error().is_none() && state.retry_in().is_none());
        assert!(state.bound(), "the binding found is kept until the next fetch");
    }
}
