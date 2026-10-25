//! Azure's retail prices, fetched in the background. They need no credential, so
//! agents' offer requests get them on every machine. New cloud offers Azure only on a
//! machine whose cloud settings have an `azure` section with a readable token, and
//! only the VM sizes and regions that section allows.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished, spawn};
use horizon_core::cloud_runtime::{
    Cancellation,
    prices::{self, AzureCatalog},
};
use std::{
    path::Path,
    sync::mpsc::channel,
    time::{Duration, Instant, SystemTime},
};

/// Retail prices change about once a month, so an hour-old catalog is current.
const FRESH: Duration = Duration::from_hours(1);

/// The VM sizes and regions of the machine's `azure` settings, in their order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct Policy {
    pub sizes: Vec<String>,
    pub regions: Vec<String>,
}

/// What the settings say: no `azure` section, its policy, or why its token cannot be read.
type Binding = Result<Option<Policy>, String>;

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    fetched: Option<Fetched<AzureCatalog>>,
    error: Option<String>,
    failed_at: Option<Instant>,
    job: Option<Job<AzureCatalog>>,
    refreshed_after: Option<Instant>,
    /// The last finished read of the settings; `None` before one, or after it failed.
    binding: Option<Binding>,
    binding_job: Option<Job<Binding>>,
    binding_failed_at: Option<Instant>,
    /// When the settings the last read found were saved.
    settings_saved: Option<SystemTime>,
}

impl State {
    /// Reads the machine's `azure` settings when they were saved since the last read,
    /// and with a binding fetches the catalog New cloud offers from.
    pub fn request_bound(&mut self, root: &Path, ctx: &egui::Context) {
        if cfg!(test) {
            return;
        }
        let saved = std::fs::metadata(root.join("settings.json"))
            .and_then(|metadata| metadata.modified())
            .ok();
        let retry = self.binding_failed_at.is_some_and(|at| at.elapsed() >= RETRY_FAILED);
        if self.binding_job.is_none() && (saved != self.settings_saved || retry) {
            self.binding_failed_at = None;
            self.settings_saved = saved;
            self.binding_job = Some(spawn(root, ctx, |settings, _| {
                Ok(settings.azure.as_ref().map_or(Ok(None), |azure| {
                    azure
                        .credential()
                        .map(|_| {
                            Some(Policy {
                                sizes: azure.vm_sizes.clone(),
                                regions: azure.regions.clone(),
                            })
                        })
                        .map_err(|error| error.to_string())
                }))
            }));
        }
        if self.bound() {
            self.request(ctx);
        }
    }

    /// Reads the settings again: a credential was just saved.
    pub fn restart(&mut self) {
        self.binding_job = None;
        self.settings_saved = None;
        self.binding_failed_at = None;
    }

    /// Whether this machine has an `azure` section, as the last read found, even when
    /// its token could not be read.
    pub fn bound(&self) -> bool {
        matches!(self.binding, Some(Ok(Some(_)) | Err(_)))
    }

    /// Whether the last read of the settings found no `azure` section.
    pub fn absent(&self) -> bool {
        matches!(self.binding, Some(Ok(None)))
    }

    /// The sizes and regions New cloud may offer, once the token was read.
    pub fn policy(&self) -> Option<&Policy> {
        self.binding.as_ref()?.as_ref().ok()?.as_ref()
    }

    /// Why Azure cannot be used now: an unreadable token, or prices that could not be
    /// fetched, for a machine with an `azure` section.
    pub fn problem(&self) -> Option<&str> {
        match &self.binding {
            Some(Err(error)) => Some(error),
            Some(Ok(Some(_))) => self.error.as_deref(),
            _ => None,
        }
    }

    /// The catalog and the policy New cloud offers Azure workers from: the token was
    /// read, and the catalog is current or being refreshed, with no failure since.
    pub fn offered(&self) -> Option<(&AzureCatalog, &Policy)> {
        let policy = self.policy()?;
        let fetched = self.fetched.as_ref()?;
        let current = self.job.is_some()
            || fetched.at.elapsed() < FRESH && self.refreshed_after.is_none_or(|refresh| fetched.at >= refresh);
        (current && self.error.is_none()).then_some((&fetched.value, policy))
    }

    /// Whether the catalog on show is older than the limit for starting a cloud.
    pub fn too_old(&self) -> bool {
        self.fetched
            .as_ref()
            .is_some_and(|fetched| fetched.at.elapsed() >= super::START_LIMIT)
    }

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
        match finished(&mut self.binding_job) {
            Some(Ok(Fetched { value, .. })) => self.binding = Some(value),
            // Unreadable settings are reported by the checks; Azure is not offered meanwhile.
            Some(Err(_)) => {
                self.binding = None;
                self.binding_failed_at = Some(Instant::now());
            }
            None => {}
        }
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

    /// As if the settings had an `azure` section with these sizes and regions, in
    /// their order, and the price list had just answered.
    pub fn answered_with_policy(&mut self, catalog: AzureCatalog, sizes: &[&str], regions: &[&str]) {
        self.answered(catalog);
        let names = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
        self.binding = Some(Ok(Some(Policy {
            sizes: names(sizes),
            regions: names(regions),
        })));
    }

    /// As if the settings had been read and had no `azure` section.
    pub fn without_section(&mut self) {
        self.binding = Some(Ok(None));
    }

    /// As if the settings had an `azure` section whose token could not be read.
    pub fn unreadable(&mut self, error: &str) {
        self.binding = Some(Err(error.to_owned()));
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
        assert_eq!(sections[0]["deployable"], true);
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
