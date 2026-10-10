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
            .push((request.panel_local_id.clone(), request.visible));
        let published = request.clone();
        let applied = request.clone();
        self.browser_create_host.io.then(
            move || publish_visibility(&published, original_visible, &workspace),
            move |app, published| app.finish_visibility_request(&applied, original_visible, published),
        );
    }

    /// The panel's visibility before the request and its workspace stamp,
    /// when the request may change it. A request that runs for the same
    /// panel already sets that visibility first.
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
        let before = self
            .browser_create_host
            .visibility_in_flight
            .iter()
            .rev()
            .find(|(local_id, _)| *local_id == request.panel_local_id)
            .map_or(panel.visible, |(_, visible)| *visible);
        Ok((before, workspace))
    }

    /// Shows or hides the panel once its manifest, audit and result say so.
    /// The board as it is now decides where the panel is; the next stamp writes
    /// that placement, including a move made while the request ran. A panel
    /// that left the agent's workspace, or that the user showed or hid while
    /// the request ran, keeps what the board says: the user's change wins, and
    /// the next stamp writes it to the manifest.
    fn finish_visibility_request(
        &mut self,
        request: &BrowserVisibilityRequest,
        original_visible: bool,
        published: Option<bool>,
    ) {
        let host = &mut self.browser_create_host;
        if let Some(index) = host
            .visibility_in_flight
            .iter()
            .position(|(local_id, _)| *local_id == request.panel_local_id)
        {
            host.visibility_in_flight.remove(index);
        }
        host.forget_stamped_placement();
        match published {
            Some(true) => {}
            Some(false) => return,
            None => {
                self.fail_visibility(
                    request,
                    "manifest_update_failed",
                    "browser panel visibility could not be updated",
                );
                return;
            }
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            return;
        };
        let still_applies = self.board.panel(panel_id).is_some_and(|panel| {
            panel.visible == original_visible
                && actor_panel(&self.board, &request.actor)
                    .is_some_and(|actor_panel| actor_panel.workspace_id == panel.workspace_id)
        });
        if !still_applies {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId, PanelOptions, WorkspaceId, browser_actor};

    /// An agent and a shown browser panel in one workspace, and another
    /// workspace. Returns the agent's actor name.
    fn fixture() -> (tempfile::TempDir, HorizonApp, String, WorkspaceId) {
        let (temp, mut app) = test_app();
        let workspace = app.board.create_workspace("agent");
        let other = app.board.create_workspace("other");
        let agent = app
            .board
            .create_panel(
                PanelOptions {
                    command: Some("/bin/sh".into()),
                    args: vec!["-c".into(), "exit 0".into()],
                    kind: PanelKind::Codex,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
        let browser = Panel::from_content(
            PanelId(900),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        app.board.panels.push(browser);
        app.board.assign_panel_to_workspace(PanelId(900), workspace);
        (temp, app, actor, other)
    }

    fn request(app: &HorizonApp, actor: &str, visible: bool) -> BrowserVisibilityRequest {
        let panel = app.board.panel(PanelId(900)).expect("browser");
        serde_json::from_value(serde_json::json!({
            "request_id": "visibility-fixture",
            "actor": actor,
            "panel_local_id": panel.local_id,
            "visible": visible,
            "requested_at_millis": 0,
            "deadline_at_millis": i64::MAX,
        }))
        .expect("request")
    }

    fn visible(app: &HorizonApp) -> bool {
        app.board.panel(PanelId(900)).expect("browser").visible
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_published_change_leaves_a_panel_that_left_the_workspace_or_that_the_user_changed() {
        let (_temp, mut app, actor, other) = fixture();
        let hide = request(&app, &actor, false);
        let workspace = app.board.panel(PanelId(900)).expect("browser").workspace_id;
        app.board.assign_panel_to_workspace(PanelId(900), other);
        app.finish_visibility_request(&hide, true, Some(true));
        assert!(visible(&app), "a panel outside the agent's workspace is not hidden");

        app.board.assign_panel_to_workspace(PanelId(900), workspace);
        let show = request(&app, &actor, true);
        app.board.set_panel_visible(PanelId(900), false);
        app.finish_visibility_request(&show, true, Some(true));
        assert!(!visible(&app), "the user hid the panel while the request ran");

        app.finish_visibility_request(&show, false, Some(true));
        assert!(visible(&app), "a request that still applies shows the panel");
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_request_after_one_that_runs_starts_from_the_visibility_it_sets() {
        let (_temp, mut app, actor, _) = fixture();
        let hide = request(&app, &actor, false);
        let actor_panel = actor_panel(&app.board, &actor).expect("actor");
        let (before, _) = app.visibility_target(&hide, actor_panel).expect("target");
        assert!(before, "the panel is shown");
        app.browser_create_host
            .visibility_in_flight
            .push((hide.panel_local_id.clone(), false));
        let show = request(&app, &actor, true);
        let (before, _) = app.visibility_target(&show, actor_panel).expect("target");
        assert!(!before, "the hide that runs first sets what a rollback restores");
    }
}
