//! Answers to `browser_provider_usage` and `browser_provider_devices`, and the
//! refusal of the requests that only a Horizon host can answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use horizon_browser::provider_catalog::{self, CatalogCache};
use horizon_browser::provider_usage::{UsageAdapter, fetch_usage};
use horizon_browser_control::manifest::{
    self,
    provider_usage::{self, ProviderUsageSummary, UsageRequest, UsageResult},
};

use super::config::HostConfig;

/// At most this many usage samples run at the same time.
const USAGE_WORKERS: usize = 4;

#[derive(Default)]
pub struct Usage {
    pub catalog: CatalogCache,
    pending: Vec<UsageRequest>,
    workers: Arc<AtomicUsize>,
}

impl Usage {
    pub fn answer(&mut self, config: &Arc<HostConfig>, host: &str, actor: &str) {
        if let Ok(requests) = provider_usage::claim_provider_usage_requests(host) {
            for request in requests {
                if request.actor != actor {
                    complete(&request.result(Vec::new(), Some("provider_usage_unavailable".into())));
                } else if let Some(error) = host_only_refusal(&request) {
                    complete(&request.result(Vec::new(), Some(error.into())));
                } else if request.catalog.is_some() {
                    self.pending.push(request);
                } else {
                    self.sample(config, request);
                }
            }
        }
        self.catalog_pages(config);
    }

    fn sample(&self, config: &Arc<HostConfig>, request: UsageRequest) {
        if let Some(name) = &request.provider
            && config.provider(name).is_err()
        {
            complete(&request.result(Vec::new(), Some("provider_unknown".into())));
            return;
        }
        if self
            .workers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < USAGE_WORKERS).then_some(n + 1)
            })
            .is_err()
        {
            complete(&request.result(Vec::new(), Some("Remote usage is busy; retry later".into())));
            return;
        }
        let permit = Permit(Arc::clone(&self.workers));
        let config = Arc::clone(config);
        std::thread::spawn(move || {
            let _permit = permit;
            let providers = summaries(&config, request.provider.as_deref());
            complete(&request.result(providers, None));
        });
    }

    fn catalog_pages(&mut self, config: &HostConfig) {
        self.catalog.poll();
        for request in std::mem::take(&mut self.pending) {
            let Some(query) = request.catalog.as_ref() else {
                continue;
            };
            let mut result = request.result(Vec::new(), None);
            if manifest::now_millis() >= request.deadline_at_millis.saturating_sub(1000) {
                result.error = Some("provider_catalog_timed_out".into());
            } else {
                match self.page(config, query) {
                    Ok(Some(page)) => result.catalog = Some(page),
                    Ok(None) => {
                        self.pending.push(request);
                        continue;
                    }
                    Err(error) => result.error = Some(error),
                }
            }
            complete(&result);
        }
    }

    fn page(
        &mut self,
        config: &HostConfig,
        query: &provider_catalog::CatalogQuery,
    ) -> Result<Option<provider_catalog::CatalogPage>, String> {
        if !query.valid() {
            return Err("Invalid provider catalog query".into());
        }
        let profile = config.provider(&query.provider).map_err(|error| error.to_string())?;
        if self.catalog.needs_refresh(&query.provider, profile) {
            let header = config
                .authorization(&query.provider)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "credentials_not_ready: the provider has no authentication".to_owned())?;
            let provider = query.provider.clone();
            let cloned = profile.clone();
            self.catalog.start(&query.provider, profile, move |progress| {
                provider_catalog::fetch(&provider, &cloned, &header, progress)
            });
        }
        self.catalog.page(profile, query).map_err(|error| error.to_string())
    }
}

/// One running usage sample; dropping it frees its place.
struct Permit(Arc<AtomicUsize>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// One summary for each configured provider, or for the one named.
fn summaries(config: &HostConfig, selected: Option<&str>) -> Vec<ProviderUsageSummary> {
    config
        .remote
        .providers
        .iter()
        .filter(|(name, _)| selected.is_none_or(|selected| selected == name.as_str()))
        .map(|(name, profile)| {
            let adapter = UsageAdapter::for_profile(profile).filter(|adapter| {
                // Usage authorization is sent only to the provider's own API.
                adapter.authorizes_origin(&profile.endpoint.origin())
            });
            let sample = adapter.map(|adapter| {
                config
                    .authorization(name)
                    .map_err(|error| error.to_string())
                    .and_then(|header| header.ok_or_else(|| "credentials_not_ready".to_owned()))
                    .and_then(|header| {
                        fetch_usage(adapter, &adapter.endpoint(), &header).map_err(|error| error.to_string())
                    })
            });
            let (usage, error) = match sample {
                Some(Ok(usage)) => (Some(usage), None),
                Some(Err(error)) => (None, Some(error)),
                None => (None, None),
            };
            ProviderUsageSummary {
                provider: name.clone(),
                supported: adapter.is_some(),
                local_session_limit: profile.local_session_limit(),
                running: usage.map(|usage| usage.running),
                allowed: usage.map(|usage| usage.allowed),
                queued: usage.map(|usage| usage.queued),
                sampled_at_millis: usage.map(|_| manifest::now_millis()),
                error,
            }
        })
        .collect()
}

/// The refusal of a request that only a Horizon host answers.
fn host_only_refusal(request: &UsageRequest) -> Option<&'static str> {
    if request.cloud_offers.is_some() {
        Some("cloud_offers_unavailable: requires a Horizon host")
    } else if request.cloud_companion.is_some() {
        Some("cloud_companion_unavailable: requires a Horizon host")
    } else if request.cloud_list.is_some() {
        Some("cloud_list_unavailable: requires a Horizon agent panel")
    } else if request.upkeep.is_some() {
        Some("upkeep_unavailable: requires a Horizon agent panel on the person's computer")
    } else if request.tailnet_credential.is_some() {
        Some("tailnet_credential_unavailable: requires the desktop owner")
    } else if request.image_destination.is_some() {
        Some("image_destination_unavailable: requires a Horizon agent panel on the person's computer")
    } else {
        None
    }
}

fn complete(result: &UsageResult) {
    if let Err(error) = provider_usage::complete_provider_usage(result) {
        tracing::warn!(kind = ?error.kind(), "could not answer a provider usage request");
    }
}
