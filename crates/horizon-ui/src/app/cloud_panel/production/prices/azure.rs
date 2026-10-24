//! Azure's retail prices, fetched in the background for agents' offer requests. They
//! need no credential, so every machine has them. Horizon cannot create an Azure cloud
//! yet and New cloud has no Azure settings, so only `cloud_offers` asks for them.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished};
use horizon_core::cloud_runtime::{
    Cancellation,
    prices::{self, AzureCatalog},
};
use std::{
    sync::mpsc::channel,
    time::{Duration, Instant},
};

/// Retail prices change about once a month, so an hour-old catalog is current.
const FRESH: Duration = Duration::from_hours(1);

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    fetched: Option<Fetched<AzureCatalog>>,
    error: Option<String>,
    failed_at: Option<Instant>,
    job: Option<Job<AzureCatalog>>,
    refreshed_after: Option<Instant>,
}

impl State {
    /// Fetches the catalog when there is none or it is older than [`FRESH`]. A failed
    /// fetch is asked again once it is older than [`RETRY_FAILED`].
    pub fn request(&mut self, ctx: &egui::Context) {
        // UI tests must never reach the price list.
        if cfg!(test) || self.job.is_some() || self.fresh().is_some() {
            return;
        }
        if self.failed_at.is_some_and(|at| at.elapsed() < RETRY_FAILED) {
            return;
        }
        self.error = None;
        self.failed_at = None;
        let (sender, receiver) = channel();
        self.job = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = prices::azure_catalog(&Cancellation::default())
                .map(|value| Fetched {
                    value,
                    at: Instant::now(),
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        match finished(&mut self.job) {
            Some(Ok(fetched)) => {
                self.fetched = Some(fetched);
                self.error = None;
                self.failed_at = None;
            }
            // The last catalog is kept, but it is never offered as current.
            Some(Err(error)) => {
                self.error = Some(error);
                self.failed_at = Some(Instant::now());
            }
            None => {}
        }
    }

    pub fn refresh(&mut self) {
        self.refreshed_after = Some(Instant::now());
        self.error = None;
        self.failed_at = None;
    }

    fn fresh(&self) -> Option<&Fetched<AzureCatalog>> {
        self.fetched.as_ref().filter(|fetched| {
            fetched.at.elapsed() < FRESH && self.refreshed_after.is_none_or(|refresh| fetched.at >= refresh)
        })
    }

    /// Azure's part of a `cloud_offers` answer, in US dollars and not deployable: the
    /// reason when the fetch failed, and offers from a current catalog. `None` while the
    /// catalog is being fetched and more than [`ANSWER_MARGIN_MILLIS`] remain until
    /// `deadline_in_millis`; closer to the deadline Azure is reported as still being
    /// fetched, so the rest of the answer is not lost.
    pub fn sections(
        &self,
        requirements: &horizon_core::cloud_runtime::offers::Requirements,
        deadline_in_millis: i64,
    ) -> Option<Vec<serde_json::Value>> {
        const LABEL: &str = horizon_core::cloud_runtime::provider::AZURE.label;
        if requirements.gpu {
            return Some(Vec::new());
        }
        if self.job.is_some() {
            if deadline_in_millis > ANSWER_MARGIN_MILLIS {
                return None;
            }
            return Some(vec![serde_json::json!({
                "provider": LABEL,
                "error": "cloud_offers_unavailable: Azure prices are still being fetched",
            })]);
        }
        if let Some(error) = &self.error {
            return Some(vec![
                serde_json::json!({"provider": LABEL, "error": format!("cloud_offers_unavailable: {error}")}),
            ]);
        }
        let Some(fetched) = self.fresh() else {
            // Nothing asked yet, as in tests, which never reach the price list.
            return Some(Vec::new());
        };
        let mut section = horizon_core::cloud_runtime::offers::azure_section(&fetched.value, requirements);
        let observed = std::time::SystemTime::now()
            .checked_sub(fetched.at.elapsed())
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|at| u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
        section["observed_at_millis"] = serde_json::json!(observed);
        section["observed_seconds_ago"] = serde_json::json!(fetched.at.elapsed().as_secs());
        Some(vec![section])
    }
}

/// A catalog as if the price list had just answered, for the tests, which never reach it.
#[cfg(test)]
impl State {
    pub fn answered(&mut self, catalog: AzureCatalog) {
        self.fetched = Some(Fetched {
            value: catalog,
            at: Instant::now(),
        });
        self.error = None;
        self.failed_at = None;
    }

    pub fn pending(&mut self) -> std::sync::mpsc::Sender<Result<Fetched<AzureCatalog>, String>> {
        let (sender, receiver) = channel();
        self.job = Some(receiver);
        sender
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::offers::Requirements;

    fn catalog() -> AzureCatalog {
        match serde_json::from_value(serde_json::json!({
            "offers": [{"size": "Standard_D4s_v3", "region": "northeurope", "vcpu": 4, "memory_gb": 16,
                "hourly_usd": 0.214}],
            "disk_month_usd": {"northeurope": {"32": 5.28}},
            "static_ip_hour_usd": {"northeurope": 0.005},
            "regions": {"northeurope": "EUROPE"},
        })) {
            Ok(catalog) => catalog,
            Err(error) => panic!("catalog: {error}"),
        }
    }

    #[test]
    fn a_failed_pending_or_refreshed_catalog_is_never_offered_as_current() {
        let requirements = Requirements::default();
        let mut state = State::default();
        assert_eq!(state.sections(&requirements, 10_000), Some(Vec::new()));
        state.answered(catalog());
        let sections = state.sections(&requirements, 10_000).unwrap_or_default();
        assert_eq!(sections[0]["provider"], "Azure");
        assert_eq!(sections[0]["deployable"], false);
        assert_eq!(sections[0]["offers"][0]["rentable"], false);
        assert!(sections[0]["observed_at_millis"].is_u64());

        let sender = state.pending();
        assert_eq!(state.sections(&requirements, 10_000), None, "waits while time remains");
        let near = state.sections(&requirements, 1_000).unwrap_or_default();
        assert!(
            near[0]["error"]
                .as_str()
                .is_some_and(|error| error.contains("still being fetched"))
        );
        assert!(sender.send(Err("Provider transport failed".into())).is_ok());
        state.poll();
        let failed = state.sections(&requirements, 10_000).unwrap_or_default();
        assert_eq!(failed[0]["provider"], "Azure");
        assert!(
            failed[0]["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("cloud_offers_unavailable"))
        );
        assert!(
            failed[0].get("offers").is_none(),
            "old prices are not offered after a failure"
        );

        state.answered(catalog());
        state.refresh();
        assert_eq!(state.sections(&requirements, 10_000), Some(Vec::new()));
        let gpu = Requirements {
            gpu: true,
            ..Requirements::default()
        };
        state.answered(catalog());
        assert_eq!(state.sections(&gpu, 10_000), Some(Vec::new()));
    }
}
