//! Host bridge for read-only `cloud_offers` requests from agents. Offers are ranked from
//! the prices New cloud keeps, fetched when missing or older than 15 minutes. Nothing is
//! rented, and without current prices a request fails rather than returning old ones.
use super::{HorizonApp, browser_requests::actor_panel};
use horizon_core::browser::manifest::{
    self,
    provider_usage::{UsageRequest, UsageResult},
};

impl HorizonApp {
    /// Answers claimed `cloud_offers` requests. Returns whether any was answered.
    pub(super) fn poll_cloud_offers(&mut self, ctx: &egui::Context) -> bool {
        let pending = std::mem::take(&mut self.browser_create_host.cloud_offers);
        let mut changed = false;
        for request in pending {
            let Some(result) = self.cloud_offers_result(&request, ctx) else {
                self.browser_create_host.cloud_offers.push(request);
                continue;
            };
            match manifest::provider_usage::complete_provider_usage(&result) {
                Ok(()) => changed = true,
                // The agent's request times out; nothing else depends on it.
                Err(error) => tracing::warn!(kind = ?error.kind(), "could not publish cloud offers"),
            }
        }
        changed
    }

    /// The answer to `request`, or `None` while prices are still being fetched.
    fn cloud_offers_result(&mut self, request: &UsageRequest, ctx: &egui::Context) -> Option<UsageResult> {
        let answer = if request.host_instance != manifest::host_instance()
            || actor_panel(&self.board, &request.actor).is_none()
        {
            Err("cloud_offers_unavailable".to_owned())
        } else if manifest::now_millis() >= request.deadline_at_millis.saturating_sub(1_000) {
            Err("cloud_offers_timed_out: prices did not arrive in time".to_owned())
        } else {
            self.cloud_offers_answer(request, ctx)?
        };
        let mut result = request.result(Vec::new(), None);
        match answer {
            Ok(offers) => result.offers = Some(offers),
            Err(error) => result.error = Some(error),
        }
        Some(result)
    }

    #[cfg(not(feature = "cloud-workspaces"))]
    #[allow(clippy::unused_self)]
    fn cloud_offers_answer(
        &mut self,
        _request: &UsageRequest,
        _ctx: &egui::Context,
    ) -> Option<Result<serde_json::Value, String>> {
        Some(Err(
            "cloud_offers_unavailable: this Horizon build has no cloud support".to_owned()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_live_agent_panel_of_this_host_gets_offers() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        let request = |host: &str, deadline: i64| -> UsageRequest {
            serde_json::from_value(serde_json::json!({
                "request_id": "offers", "actor": "horizon:nobody", "host_instance": host,
                "deadline_at_millis": deadline, "cloud_offers": {}, "claimed": true
            }))
            .unwrap()
        };
        let other_host = app.cloud_offers_result(&request("other-host", i64::MAX), &ctx).unwrap();
        assert_eq!(other_host.error.as_deref(), Some("cloud_offers_unavailable"));
        assert!(other_host.offers.is_none());
        // An actor without a panel on this host is refused before anything is priced.
        let unknown = app
            .cloud_offers_result(&request(manifest::host_instance(), i64::MAX), &ctx)
            .unwrap();
        assert_eq!(unknown.error.as_deref(), Some("cloud_offers_unavailable"));
        assert!(app.browser_create_host.cloud_offers.is_empty());
    }
}
