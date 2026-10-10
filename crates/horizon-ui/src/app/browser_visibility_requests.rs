//! Host-side handling for agent-requested browser panel visibility changes.
//! The board decides whether a request may apply. The coordination worker
//! then audits it and publishes the manifest's visibility and the result, and
//! a later frame shows or hides the panel once all of that succeeded, so a
//! slow disk never stalls a frame.

use horizon_core::PanelKind;
use horizon_core::browser::manifest::{
    self, BrowserVisibilityAuditStatus, BrowserVisibilityRequest, BrowserVisibilityResult, ManifestWorkspace,
};

use super::HorizonApp;
use super::browser_requests::{ActorPanel, actor_panel, browser_workspace, publish_manifest_host_state};

type Refusal = (&'static str, &'static str);

impl HorizonApp {
    pub(super) fn apply_browser_visibility_request(
        &mut self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) {
        let (original_visible, workspace) = match self.visibility_target(request, actor_panel) {
            Ok(target) => target,
            Err((code, message)) => {
                self.fail_visibility(request, code, message);
                return;
            }
        };
        let owned_by_actor = manifest::read(&request.panel_local_id)
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        if !owned_by_actor {
            self.fail_visibility(request, "ownership_changed", "browser panel ownership changed");
            return;
        }
        // No stamp writes this manifest until the request ends, so its
        // visibility changes only through the audited transaction.
        self.browser_create_host
            .visibility_in_flight
            .push(request.panel_local_id.clone());
        let published = request.clone();
        let applied = request.clone();
        self.browser_create_host.io.then(
            move || publish_visibility(&published, original_visible, &workspace),
            move |app, published| app.finish_visibility_request(&applied, published),
        );
    }

    /// The panel's visibility before the request and its workspace stamp,
    /// when the request may change it.
    fn visibility_target(
        &self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) -> Result<(bool, ManifestWorkspace), Refusal> {
        if request.deadline_at_millis < manifest::now_millis() {
            return Err(("request_expired", "browser visibility request expired"));
        }
        let panel_id = self.board.panel_id_by_local_id(&request.panel_local_id).ok_or((
            "panel_not_in_host",
            "browser panel is not hosted by the requesting agent's Horizon instance",
        ))?;
        let panel = self
            .board
            .panel(panel_id)
            .ok_or(("panel_closed", "browser panel is not live"))?;
        if panel.kind != PanelKind::Browser {
            return Err(("not_browser_panel", "target panel is not a browser panel"));
        }
        if panel.workspace_id != actor_panel.workspace_id {
            return Err((
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            ));
        }
        let workspace = browser_workspace(&self.board, panel.workspace_id)
            .ok_or(("workspace_unavailable", "browser panel workspace is not live"))?;
        Ok((panel.visible, workspace))
    }

    /// Shows or hides the panel once its manifest, audit and result say so.
    fn finish_visibility_request(&mut self, request: &BrowserVisibilityRequest, published: bool) {
        let in_flight = &mut self.browser_create_host.visibility_in_flight;
        if let Some(index) = in_flight
            .iter()
            .position(|local_id| *local_id == request.panel_local_id)
        {
            in_flight.remove(index);
        }
        if !published {
            return;
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            return;
        };
        let changed = self.board.set_panel_visible(panel_id, request.visible);
        if !request.visible && self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        if !request.visible
            && self.board.focused.is_none()
            && let Some(actor_panel) = actor_panel(&self.board, &request.actor)
        {
            self.board.focus(actor_panel.panel_id);
        }
        if changed {
            self.mark_runtime_dirty();
        }
    }

    /// Publishes a refused visibility request on the coordination worker.
    pub(super) fn fail_visibility(&mut self, request: &BrowserVisibilityRequest, code: &str, message: &str) {
        let request = request.clone();
        let result = BrowserVisibilityResult::failed(&request, code, message);
        self.browser_create_host
            .io
            .write(move || complete_visibility_failure(&request, &result));
    }
}

/// Audits the request and publishes the manifest's new visibility and the
/// result, on the coordination worker. A completion that cannot be audited
/// restores the manifest. Says whether the change was published.
fn publish_visibility(
    request: &BrowserVisibilityRequest,
    original_visible: bool,
    workspace: &ManifestWorkspace,
) -> bool {
    let refuse = |code, message| {
        complete_visibility_failure(request, &BrowserVisibilityResult::failed(request, code, message));
        false
    };
    if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Dispatched) {
        tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility dispatch");
        return refuse("audit_failed", "Horizon refused an unaudited visibility change");
    }
    if let Err(error) = publish_manifest_host_state(&request.panel_local_id, request.visible, workspace) {
        tracing::warn!(request_id = %request.request_id, %error, "could not update browser manifest visibility");
        return refuse(
            "manifest_update_failed",
            "browser panel visibility could not be updated",
        );
    }
    if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Completed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility completion");
        let _ = publish_manifest_host_state(&request.panel_local_id, original_visible, workspace);
        return refuse("audit_failed", "visibility change could not be audited");
    }
    complete_visibility_result(&BrowserVisibilityResult::ready(request));
    true
}

fn complete_visibility_failure(request: &BrowserVisibilityRequest, result: &BrowserVisibilityResult) {
    if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser visibility audit");
    }
    complete_visibility_result(result);
}

fn complete_visibility_result(result: &BrowserVisibilityResult) {
    if let Err(error) = manifest::complete_visibility_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser visibility result");
    }
}
