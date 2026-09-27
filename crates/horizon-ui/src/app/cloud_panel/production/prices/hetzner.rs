//! Hetzner's catalog, fetched beside the `RunPod` price list when this machine has a
//! Hetzner binding, for agents' offer requests and the prices sent to workers.
use super::{Fetched, Job, RETRY_FAILED, finished, spawn, stale};
use horizon_core::cloud_runtime::prices::{self, HetznerCatalog};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Longest wait for a running Hetzner fetch before prices go to workers without it.
const WAIT_FOR_FETCH: Duration = Duration::from_secs(20);
/// An agent's answer waits for a running Hetzner fetch only while more than this is
/// left before its deadline; after that it reports Hetzner as still being fetched.
const ANSWER_MARGIN_MILLIS: i64 = 3_000;

#[derive(Default)]
pub(in crate::app) struct State {
    /// The latest catalog; `None` inside when this machine has no Hetzner binding.
    fetched: Option<Fetched<Option<HetznerCatalog>>>,
    /// The server types this machine's settings try, in order, from the same fetch.
    server_types: Vec<String>,
    error: Option<String>,
    failed_at: Option<Instant>,
    job: Option<Job<(Option<HetznerCatalog>, Vec<String>)>>,
    /// When the running fetch started.
    started: Option<Instant>,
}

impl State {
    /// Fetches the catalog when there is none or it is older than [`super::FRESH`]. A failed
    /// fetch is asked again once it is older than [`RETRY_FAILED`].
    pub(super) fn request(&mut self, root: &Path, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach Hetzner.
        if cfg!(test) {
            return;
        }
        if self.failed_at.is_some_and(|at| at.elapsed() >= RETRY_FAILED) {
            self.error = None;
            self.failed_at = None;
        }
        if self.job.is_none() && self.error.is_none() && self.fetched.as_ref().is_none_or(|fetched| stale(fetched.at)) {
            self.job = Some(spawn(root, ctx, |settings, cancel| {
                let server_types = settings
                    .hetzner
                    .as_ref()
                    .map(|hetzner| hetzner.server_types.clone())
                    .unwrap_or_default();
                Ok((prices::hetzner_catalog(settings, cancel)?, server_types))
            }));
            self.started = Some(Instant::now());
        }
    }

    pub(super) fn poll(&mut self) {
        let finished = finished(&mut self.job);
        if self.job.is_none() {
            self.started = None;
        }
        match finished {
            Some(Ok(Fetched {
                value: (catalog, server_types),
                at,
            })) => {
                self.fetched = Some(Fetched { value: catalog, at });
                self.server_types = server_types;
                self.error = None;
                self.failed_at = None;
            }
            // A catalog that could not be refreshed is no longer offered as current.
            Some(Err(error)) => {
                self.fetched = None;
                self.error = Some(error);
                self.failed_at = Some(Instant::now());
            }
            None => {}
        }
    }

    pub(super) fn refresh(&mut self) {
        *self = Self::default();
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

    /// The server types this machine's settings try, in order.
    pub fn server_types(&self) -> &[String] {
        &self.server_types
    }

    /// The reason the last fetch failed, while it is reported.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The current catalog, when this machine has a Hetzner binding.
    pub fn fresh(&self) -> Option<&Fetched<Option<HetznerCatalog>>> {
        self.fetched.as_ref().filter(|fetched| !stale(fetched.at))
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

/// A catalog as if Hetzner had just answered, for tests, which never contact it.
#[cfg(test)]
impl State {
    pub fn answered_with_types(&mut self, catalog: Option<HetznerCatalog>, server_types: &[&str]) {
        self.answered(catalog);
        self.server_types = server_types.iter().map(|&name| name.to_owned()).collect();
    }

    pub fn answered(&mut self, catalog: Option<HetznerCatalog>) {
        self.fetched = Some(Fetched {
            value: catalog,
            at: Instant::now(),
        });
    }

    pub fn failed(&mut self, error: &str) {
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
        state.refresh();
        assert!(state.job.is_none() && state.fresh().is_none());
    }
}
