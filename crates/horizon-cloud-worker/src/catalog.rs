//! Worker-owned device discovery continues independently of the laptop.
use horizon_browser::provider_catalog::{self, CatalogCache, CatalogPage, CatalogQuery};
use horizon_browser_control::manifest::{
    self,
    provider_usage::{UsageRequest, UsageResult},
};
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
            if request.catalog.is_none() {
                continue;
            }
            let Some(result) = self.result(&request, capabilities) else {
                self.pending.push(request);
                continue;
            };
            if manifest::provider_usage::complete_provider_usage(&result).is_err() {
                self.pending.push(request);
            }
        }
    }
    fn result(&mut self, request: &UsageRequest, capabilities: &horizon_cloud::Capabilities) -> Option<UsageResult> {
        let query = request.catalog.as_ref()?;
        let mut result = request.result(Vec::new(), None);
        if !request.actor.starts_with("horizon:cloud-") || request.host_instance != manifest::host_instance() {
            result.error = Some("provider_catalog_unavailable".into());
        } else if manifest::now_millis() >= request.deadline_at_millis.saturating_sub(1000) {
            result.error = Some("provider_catalog_timed_out".into());
        } else {
            match self.page(capabilities, query) {
                Ok(Some(page)) => result.catalog = Some(page),
                Ok(None) => {
                    return None;
                }
                Err(error) => result.error = Some(error.to_string()),
            }
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_minimum_deadline_times_out_without_requesting_a_catalog() {
        let request: UsageRequest = serde_json::from_value(serde_json::json!({
            "request_id":"expired", "actor":"horizon:cloud-fixture",
            "host_instance":manifest::host_instance(), "deadline_at_millis":i64::MIN,
            "catalog":{"provider":"fixture"}, "claimed":true
        }))
        .unwrap();
        // No remote capability or credential exists: reaching page() would fail differently.
        let result = Host::default()
            .result(&request, &horizon_cloud::Capabilities::default())
            .unwrap();
        assert_eq!(result.error.as_deref(), Some("provider_catalog_timed_out"));
        assert!(result.catalog.is_none());
    }
}
