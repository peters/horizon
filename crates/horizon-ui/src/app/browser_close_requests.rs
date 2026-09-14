//! Host-side handling for agent-requested browser panel closes. Closing a
//! panel drops its state, which stops the session and, for a remote session,
//! releases the provider allocation through the driver's own teardown.

use horizon_core::PanelKind;
use horizon_core::browser::manifest::{self, BrowserCloseAuditStatus, BrowserCloseRequest, BrowserCloseResult};

use super::HorizonApp;
use super::browser_requests::{ActorPanel, actor_panel, launched_by_this_host};

impl HorizonApp {
    pub(super) fn poll_browser_close_requests(&mut self) -> bool {
        let requests = match manifest::list_close_requests() {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser close requests");
                return false;
            }
        };
        let mut changed = false;
        for request in requests {
            if !launched_by_this_host(request.host_instance.as_deref()) {
                continue;
            }
            let Some(actor_panel) = actor_panel(&self.board, &request.actor) else {
                continue;
            };
            let request = match manifest::claim_close_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, error = %error, "could not claim browser close request");
                    continue;
                }
            };
            changed |= self.apply_browser_close_request(&request, actor_panel);
        }
        changed
    }

    fn apply_browser_close_request(&mut self, request: &BrowserCloseRequest, actor_panel: ActorPanel) -> bool {
        if request.deadline_at_millis < manifest::now_millis() {
            complete_close_failure(request, "request_expired", "browser close request expired");
            return false;
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            complete_close_failure(
                request,
                "panel_not_in_host",
                "browser panel is not hosted by the requesting agent's Horizon instance",
            );
            return false;
        };
        let Some(panel) = self.board.panel(panel_id) else {
            complete_close_failure(request, "panel_closed", "browser panel is not live");
            return false;
        };
        if panel.kind != PanelKind::Browser {
            complete_close_failure(request, "not_browser_panel", "target panel is not a browser panel");
            return false;
        }
        if panel.workspace_id != actor_panel.workspace_id {
            complete_close_failure(
                request,
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            );
            return false;
        }
        if self.browser_create_is_pending(panel_id) {
            complete_close_failure(
                request,
                "create_pending",
                "browser panel is still being created; wait for browser_create to return",
            );
            return false;
        }
        let owned_by_actor = manifest::read(&request.panel_local_id)
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        if !owned_by_actor {
            complete_close_failure(request, "ownership_changed", "browser panel ownership changed");
            return false;
        }
        if let Err(error) = manifest::record_close_status(request, BrowserCloseAuditStatus::Dispatched) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser close dispatch");
            complete_close_failure(request, "audit_failed", "Horizon refused an unaudited close");
            return false;
        }
        if self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        self.close_panel(panel_id);
        if self.board.focused.is_none() {
            self.board.focus(actor_panel.panel_id);
        }
        // The audit journal outlives the panel, so the completion is recorded
        // after the close; a failure to record it is logged, not reverted,
        // because the panel is already gone.
        if let Err(error) = manifest::record_close_status(request, BrowserCloseAuditStatus::Completed) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser close completion");
        }
        complete_close_result(&BrowserCloseResult::closed(request));
        self.mark_runtime_dirty();
        true
    }
}

fn complete_close_failure(request: &BrowserCloseRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_close_status(request, BrowserCloseAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser close audit");
    }
    complete_close_result(&BrowserCloseResult::failed(request, code, message));
}

fn complete_close_result(result: &BrowserCloseResult) {
    if let Err(error) = manifest::complete_close_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser close result");
    }
}
