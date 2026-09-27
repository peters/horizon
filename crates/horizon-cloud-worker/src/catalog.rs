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
    pub fn revoke(&mut self, fence: &Path) -> io::Result<()> {
        std::fs::write(fence, "revoked")?;
        self.cache.clear();
        Ok(())
    }

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
            // The worker's credential is part of the private configuration read
            // above, so its job starts at the provider request.
            let header = config
                .authorization
                .remove(&query.provider)
                .ok_or_else(|| io::Error::other("Remote credentials are unavailable"))?;
            let provider = query.provider.clone();
            let cloned = profile.clone();
            self.cache.start(&query.provider, profile, move |progress| {
                provider_catalog::fetch(&provider, &cloned, &header, progress)
            });
        }
        self.cache.page(profile, query).map_err(io::Error::other)
    }
    pub fn poll(&mut self, capabilities: &horizon_cloud::Capabilities) {
        let requests = std::mem::take(&mut self.pending);
        for request in requests {
            let result = if request.cloud_offers.is_some() {
                Some(super::offers::answer(&request))
            } else if request.catalog.is_some() {
                self.result(&request, capabilities)
            } else {
                continue;
            };
            let Some(result) = result else {
                self.pending.push(request);
                continue;
            };
            if manifest::provider_usage::complete_provider_usage(&result).is_err()
                && retried(&request, manifest::now_millis())
            {
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

/// Whether an answer that could not be published is tried again: cloud offers only while
/// the agent still waits for them.
fn retried(request: &UsageRequest, now: i64) -> bool {
    request.cloud_offers.is_none() || now < request.deadline_at_millis
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unpublished_answers_are_retried_while_the_agent_waits() {
        let request = |query: serde_json::Value| -> UsageRequest {
            let mut request = serde_json::json!({
                "request_id":"retry", "actor":"horizon:cloud-fixture", "host_instance":manifest::host_instance(),
                "deadline_at_millis":1_000, "claimed":true
            });
            request
                .as_object_mut()
                .unwrap()
                .extend(query.as_object().unwrap().clone());
            serde_json::from_value(request).unwrap()
        };
        let offers = request(serde_json::json!({"cloud_offers": {}}));
        assert!(retried(&offers, 999));
        assert!(!retried(&offers, 1_000), "nobody waits for it after its deadline");
        let catalog = request(serde_json::json!({"catalog": {"provider": "account"}}));
        assert!(retried(&catalog, 1_000));
    }
    #[test]
    fn revocation_discards_pending_catalog_without_dropping_request_results() {
        let root = tempfile::tempdir().unwrap();
        let profile = serde_json::from_str(
            r#"{"adapter":"browserstack","endpoint":"https://hub-cloud.browserstack.com/wd/hub"}"#,
        )
        .unwrap();
        let mut host = Host::default();
        let (release, wait) = std::sync::mpsc::channel();
        host.cache.start("account", &profile, move |_| {
            wait.recv().unwrap();
            Ok(Vec::new())
        });
        assert!(!host.cache.needs_refresh("account", &profile));
        let request = serde_json::from_value(serde_json::json!({
            "request_id":"pending", "actor":"horizon:cloud-fixture", "host_instance":manifest::host_instance(),
            "deadline_at_millis":i64::MAX, "catalog":{"provider":"account"}, "claimed":true
        }))
        .unwrap();
        host.pending.push(request);
        assert!(host.revoke(&root.path().join("missing-parent/fence")).is_err());
        assert!(
            !host.cache.needs_refresh("account", &profile),
            "failed revocation preserves existing state"
        );
        let fence = root.path().join("revoked");
        host.revoke(&fence).unwrap();
        assert_eq!(std::fs::read_to_string(&fence).unwrap(), "revoked");
        assert_eq!(host.pending.len(), 1, "pending callers must still receive a result");
        assert!(host.cache.needs_refresh("account", &profile));
        let (second, held) = std::sync::mpsc::channel();
        host.cache.start("account", &profile, move |_| {
            held.recv().unwrap();
            Ok(Vec::new())
        });
        host.revoke(&fence).unwrap();
        assert!(
            !host.cache.needs_refresh("account", &profile),
            "revocation drops results but running jobs stay counted"
        );
        release.send(()).unwrap();
        second.send(()).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !host.cache.needs_refresh("account", &profile) {
            assert!(std::time::Instant::now() < end, "finished jobs release their budget");
            std::thread::yield_now();
        }
    }
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
