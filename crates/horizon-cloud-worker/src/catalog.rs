//! Worker-owned device discovery continues independently of the laptop.
use horizon_browser::provider_catalog::{self, CatalogCache, CatalogPage, CatalogQuery};
use horizon_browser_control::manifest::{self, provider_usage::UsageRequest};
use std::{io, path::Path};
#[derive(Default)]
pub struct Host {
    pub cache: CatalogCache,
    pub pending: Vec<UsageRequest>,
}
impl Host {
    pub fn page(
        &mut self,
        capabilities: &horizon_cloud::Capabilities,
        query: &CatalogQuery,
    ) -> io::Result<Option<CatalogPage>> {
        if !query.valid() {
            return Err(io::Error::other("Invalid provider catalog query"));
        }
        if Path::new("/run/horizon-credentials/browserstack-revoked").exists() {
            return Err(io::Error::other("Remote credential grant was revoked"));
        }
        let mut config = super::remote::configuration_for(capabilities)?;
        let profile = config
            .remote
            .providers
            .get(&query.provider)
            .ok_or_else(|| io::Error::other("Remote provider is not granted to this cloud"))?;
        self.cache.poll();
        if self.cache.needs_refresh(&query.provider, profile) {
            let header = config
                .authorization
                .remove(&query.provider)
                .ok_or_else(|| io::Error::other("Remote credentials are unavailable"))?;
            let provider = query.provider.clone();
            let cloned = profile.clone();
            self.cache.start(&query.provider, profile, move || {
                provider_catalog::fetch(&provider, &cloned, &header)
            });
        }
        self.cache.page(profile, query).map_err(io::Error::other)
    }
    pub fn poll(&mut self, capabilities: &horizon_cloud::Capabilities) {
        let requests = std::mem::take(&mut self.pending);
        for request in requests {
            let Some(query) = &request.catalog else { continue };
            let mut result = request.result(Vec::new(), None);
            if !request.actor.starts_with("horizon:cloud-") || request.host_instance != manifest::host_instance() {
                result.error = Some("provider_catalog_unavailable".into());
            } else if manifest::now_millis() >= request.deadline_at_millis - 1000 {
                result.error = Some("provider_catalog_timed_out".into());
            } else {
                match self.page(capabilities, query) {
                    Ok(Some(page)) => result.catalog = Some(page),
                    Ok(None) => {
                        self.pending.push(request);
                        continue;
                    }
                    Err(error) => result.error = Some(error.to_string()),
                }
            }
            if manifest::provider_usage::complete_provider_usage(&result).is_err() {
                self.pending.push(request);
            }
        }
    }
}
