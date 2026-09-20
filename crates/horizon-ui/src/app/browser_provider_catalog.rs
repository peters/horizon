//! UI and public MCP/CLI discovery share one account-bound cache.
use super::{HorizonApp, browser_requests::actor_panel};
use horizon_core::browser::{
    manifest::{self, provider_usage::UsageRequest},
    remote_catalog::{self, CatalogCache},
};
#[derive(Default)]
pub(super) struct CatalogHostState {
    pub(super) cache: CatalogCache,
    pub(super) pending: Vec<UsageRequest>,
}
impl HorizonApp {
    pub(super) fn poll_provider_catalog(&mut self) -> bool {
        let requests = std::mem::take(&mut self.browser_create_host.catalog.pending);
        self.browser_create_host.catalog.cache.poll();
        let mut changed = false;
        for request in requests {
            let Some(query) = &request.catalog else { continue };
            let mut result = request.result(Vec::new(), None);
            if request.host_instance != manifest::host_instance() || actor_panel(&self.board, &request.actor).is_none()
            {
                result.error = Some("provider_catalog_unavailable".into());
            } else if manifest::now_millis() >= request.deadline_at_millis - 1000 {
                result.error = Some("provider_catalog_timed_out".into());
            } else if let Some(profile) = self.template_config.browser.remote.providers.get(&query.provider) {
                let cache = &mut self.browser_create_host.catalog.cache;
                remote_catalog::refresh(cache, &query.provider, profile, &self.remote_browser_credentials);
                match cache.page(profile, query) {
                    Ok(Some(page)) => result.catalog = Some(page),
                    Ok(None) => {
                        self.browser_create_host.catalog.pending.push(request);
                        continue;
                    }
                    Err(error) => result.error = Some(error.to_string()),
                }
            } else {
                result.error = Some("provider_unknown".into());
            }
            if manifest::provider_usage::complete_provider_usage(&result).is_ok() {
                changed = true;
            } else {
                self.browser_create_host.catalog.pending.push(request);
            }
        }
        changed
    }
}
