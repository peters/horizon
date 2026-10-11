//! Host-side handling for agent-requested browser panel visibility changes.

use horizon_core::PanelKind;
use horizon_core::browser::manifest::{
    self, BrowserVisibilityAuditStatus, BrowserVisibilityRequest, BrowserVisibilityResult,
};

use super::HorizonApp;
use super::browser_requests::{ActorPanel, browser_workspace, publish_manifest_host_state};

impl HorizonApp {
    pub(super) fn apply_browser_visibility_request(
        &mut self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) {
        if request.deadline_at_millis < manifest::now_millis() {
            complete_visibility_failure(request, "request_expired", "browser visibility request expired");
            return;
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            complete_visibility_failure(
                request,
                "panel_not_in_host",
                "browser panel is not hosted by the requesting agent's Horizon instance",
            );
            return;
        };
        let Some(panel) = self.board.panel(panel_id) else {
            complete_visibility_failure(request, "panel_closed", "browser panel is not live");
            return;
        };
        if panel.kind != PanelKind::Browser {
            complete_visibility_failure(request, "not_browser_panel", "target panel is not a browser panel");
            return;
        }
        if panel.workspace_id != actor_panel.workspace_id {
            complete_visibility_failure(
                request,
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            );
            return;
        }
        let Some(workspace) = browser_workspace(&self.board, panel.workspace_id) else {
            complete_visibility_failure(request, "workspace_unavailable", "browser panel workspace is not live");
            return;
        };
        let original_visible = panel.visible;
        let owned_by_actor = manifest::read(&request.panel_local_id)
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        if !owned_by_actor {
            complete_visibility_failure(request, "ownership_changed", "browser panel ownership changed");
            return;
        }
        if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Dispatched) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility dispatch");
            complete_visibility_failure(
                request,
                "audit_failed",
                "Horizon refused an unaudited visibility change",
            );
            return;
        }
        if let Err(error) = publish_manifest_host_state(&request.panel_local_id, request.visible, &workspace) {
            tracing::warn!(request_id = %request.request_id, %error, "could not update browser manifest visibility");
            complete_visibility_failure(
                request,
                "manifest_update_failed",
                "browser panel visibility could not be updated",
            );
            return;
        }
        let local_changed = self.board.set_panel_visible(panel_id, request.visible);
        if !request.visible && self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        if !request.visible && self.board.focused.is_none() {
            self.board.focus(actor_panel.panel_id);
        }
        if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Completed) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility completion");
            let _ = publish_manifest_host_state(&request.panel_local_id, original_visible, &workspace);
            let _ = self.board.set_panel_visible(panel_id, original_visible);
            complete_visibility_failure(request, "audit_failed", "visibility change could not be audited");
            return;
        }
        complete_visibility_result(&BrowserVisibilityResult::ready(request));
        if local_changed {
            self.mark_runtime_dirty();
        }
    }

    /// Publishes a refused visibility request on the coordination worker.
    pub(super) fn fail_visibility(&mut self, request: &BrowserVisibilityRequest, code: &str, message: &str) {
        let request = request.clone();
        let (code, message) = (code.to_string(), message.to_string());
        self.browser_create_host
            .io
            .write(move || complete_visibility_failure(&request, &code, &message));
    }
}

fn complete_visibility_failure(request: &BrowserVisibilityRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser visibility audit");
    }
    complete_visibility_result(&BrowserVisibilityResult::failed(request, code, message));
}

fn complete_visibility_result(result: &BrowserVisibilityResult) {
    if let Err(error) = manifest::complete_visibility_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser visibility result");
    }
}
