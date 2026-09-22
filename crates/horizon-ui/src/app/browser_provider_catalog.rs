//! UI and public MCP/CLI discovery share one account-bound cache.
use super::{HorizonApp, browser_requests::actor_panel};
use horizon_core::browser::{
    manifest::{
        self,
        provider_usage::{UsageRequest, UsageResult},
    },
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
            if request.catalog.is_none() {
                continue;
            }
            let Some(result) = self.provider_catalog_result(&request) else {
                self.browser_create_host.catalog.pending.push(request);
                continue;
            };
            if manifest::provider_usage::complete_provider_usage(&result).is_ok() {
                changed = true;
            } else {
                self.browser_create_host.catalog.pending.push(request);
            }
        }
        changed
    }
    fn provider_catalog_result(&mut self, request: &UsageRequest) -> Option<UsageResult> {
        let query = request.catalog.as_ref()?;
        let mut result = request.result(Vec::new(), None);
        if request.host_instance != manifest::host_instance() || actor_panel(&self.board, &request.actor).is_none() {
            result.error = Some("provider_catalog_unavailable".into());
        } else if manifest::now_millis() >= request.deadline_at_millis.saturating_sub(1000) {
            result.error = Some("provider_catalog_timed_out".into());
        } else if let Some(profile) = self.template_config.browser.remote.providers.get(&query.provider) {
            let cache = &mut self.browser_create_host.catalog.cache;
            remote_catalog::refresh(cache, &query.provider, profile, &self.remote_browser_credentials);
            match cache.page(profile, query) {
                Ok(Some(page)) => result.catalog = Some(page),
                Ok(None) => {
                    return None;
                }
                Err(error) => result.error = Some(error.to_string()),
            }
        } else {
            result.error = Some("provider_unknown".into());
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::{Board, PanelKind, PanelState, RuntimeState, WorkspaceState};
    #[test]
    fn authenticated_minimum_deadline_times_out_before_provider_lookup() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.board = Board::from_runtime_state(&RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "fixture".into(),
                panels: vec![PanelState {
                    local_id: "agent".into(),
                    kind: PanelKind::Codex,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        let request: UsageRequest = serde_json::from_value(serde_json::json!({
            "request_id":"expired", "actor":"horizon:agent",
            "host_instance":manifest::host_instance(), "deadline_at_millis":i64::MIN,
            "catalog":{"provider":"fixture"}, "claimed":true
        }))
        .unwrap();
        assert!(actor_panel(&app.board, &request.actor).is_some());
        let result = app.provider_catalog_result(&request).unwrap();
        assert_eq!(result.error.as_deref(), Some("provider_catalog_timed_out"));
        assert!(result.catalog.is_none());
    }
}
