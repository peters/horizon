//! Host-side handling for agent-requested browser panel resizes. The browser
//! viewport is the panel body, so the host sizes the panel for the requested
//! viewport and stamps the resulting viewport on the panel's manifest before
//! the result is published, so the caller can verify the viewport it asked for.

use horizon_core::browser::manifest::{self, BrowserResizeAuditStatus, BrowserResizeRequest, BrowserResizeResult};
use horizon_core::{PanelId, PanelKind, WorkspaceId};

use super::HorizonApp;
use super::browser_requests::{ActorPanel, actor_panel, browser_workspace, launched_by_this_host};
use super::browser_viewport::{panel_size_for_viewport, viewport_size_from_panel_size};

/// Why a claimed resize request is refused, as the typed result code and its
/// message. Every path leaves the panel untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ResizeRefusal {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
}

const fn refusal(code: &'static str, message: &'static str) -> ResizeRefusal {
    ResizeRefusal { code, message }
}

impl HorizonApp {
    pub(super) fn poll_browser_resize_requests(&mut self) -> bool {
        let root = self.host_manifest_root().to_path_buf();
        let requests = match manifest::list_resize_requests_in(&root) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser resize requests");
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
            let request = match manifest::claim_resize_request_in(
                &root,
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, error = %error, "could not claim browser resize request");
                    continue;
                }
            };
            changed |= self.apply_browser_resize_request(&request, actor_panel);
        }
        changed
    }

    /// Decide whether `request` may resize a panel, from board state and the
    /// live ownership the caller read from the manifest. Pure: nothing is
    /// written, so the refusal paths are testable without a driver.
    pub(super) fn resize_target(
        &self,
        request: &BrowserResizeRequest,
        actor_panel: ActorPanel,
        owned_by_actor: bool,
        now_millis: i64,
    ) -> Result<(PanelId, [f32; 2]), ResizeRefusal> {
        if request.deadline_at_millis < now_millis {
            return Err(refusal("request_expired", "browser resize request expired"));
        }
        let panel_id = self.board.panel_id_by_local_id(&request.panel_local_id).ok_or(refusal(
            "panel_not_in_host",
            "browser panel is not hosted by the requesting agent's Horizon instance",
        ))?;
        let panel = self
            .board
            .panel(panel_id)
            .ok_or(refusal("panel_closed", "browser panel is not live"))?;
        if panel.kind != PanelKind::Browser {
            return Err(refusal("not_browser_panel", "target panel is not a browser panel"));
        }
        if panel.workspace_id != actor_panel.workspace_id {
            return Err(refusal(
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            ));
        }
        if panel
            .browser()
            .is_some_and(horizon_core::browser::BrowserPanelState::is_remote)
        {
            return Err(refusal(
                "remote_viewport_fixed",
                "remote target panels have a fixed device viewport",
            ));
        }
        if !owned_by_actor {
            return Err(refusal("ownership_changed", "browser panel ownership changed"));
        }
        Ok((panel_id, panel_size_for_viewport([request.width, request.height])))
    }

    fn apply_browser_resize_request(&mut self, request: &BrowserResizeRequest, actor_panel: ActorPanel) -> bool {
        // Production resolves the session store from the same Horizon home
        // the queue lives in; tests point the store at a temp root, so every
        // manifest write here stays under it.
        let root = self.host_manifest_root().to_path_buf();
        let owned_by_actor = manifest::read_at(&manifest::manifest_path_for_root(&root, &request.panel_local_id))
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        let (panel_id, size) = match self.resize_target(request, actor_panel, owned_by_actor, manifest::now_millis()) {
            Ok(target) => target,
            Err(refused) => {
                complete_resize_failure(&root, request, refused.code, refused.message);
                return false;
            }
        };
        let Some(workspace) = browser_workspace(&self.board, actor_panel.workspace_id) else {
            complete_resize_failure(
                &root,
                request,
                "workspace_unavailable",
                "browser panel workspace is not live",
            );
            return false;
        };
        // A journal that cannot be written refuses the resize instead of
        // leaving a resized panel with no dispatch record.
        if let Err(error) = manifest::record_resize_status_in(&root, request, BrowserResizeAuditStatus::Dispatched) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser panel resize dispatch");
            complete_resize_failure(
                &root,
                request,
                "audit_failed",
                "Horizon refused an unaudited panel resize",
            );
            return false;
        }
        let original_size = self.board.panel(panel_id).map_or(size, |panel| panel.layout.size);
        let scope = self.resize_collision_scope(panel_id);
        let resized = self.board.resize_panel_with_workspace_scope(panel_id, size, &scope);
        let Some(panel) = self.board.panel(panel_id) else {
            complete_resize_failure(&root, request, "panel_closed", "browser panel is not live");
            return false;
        };
        let applied_viewport = viewport_size_from_panel_size(panel.layout.size);
        if let Err(error) = manifest::sync_host_state_in(
            &root,
            &request.panel_local_id,
            panel.visible,
            &workspace,
            Some(applied_viewport),
        ) {
            tracing::warn!(request_id = %request.request_id, %error, "could not stamp the resized browser panel");
            let _ = self
                .board
                .resize_panel_with_workspace_scope(panel_id, original_size, &scope);
            complete_resize_failure(
                &root,
                request,
                "manifest_update_failed",
                "browser panel size could not be updated",
            );
            return false;
        }
        if let Err(error) = manifest::record_resize_status_in(&root, request, BrowserResizeAuditStatus::Completed) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser panel resize completion");
            let _ = self
                .board
                .resize_panel_with_workspace_scope(panel_id, original_size, &scope);
            complete_resize_failure(&root, request, "audit_failed", "panel resize could not be audited");
            return false;
        }
        complete_resize_result(
            &root,
            &BrowserResizeResult::ready(request, applied_viewport[0], applied_viewport[1]),
        );
        if resized {
            self.mark_runtime_dirty();
        }
        resized
    }

    /// Collision scope for an agent resize: the panel's own workspace when it
    /// is detached, every attached workspace otherwise — the same scope the
    /// matching render pass resizes with.
    fn resize_collision_scope(&self, panel_id: PanelId) -> Vec<WorkspaceId> {
        let Some(workspace_id) = self.board.panel(panel_id).map(|panel| panel.workspace_id) else {
            return Vec::new();
        };
        self.workspace_collision_scope(self.workspace_is_detached(workspace_id).then_some(workspace_id))
    }
}

fn complete_resize_failure(root: &std::path::Path, request: &BrowserResizeRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_resize_status_in(root, request, BrowserResizeAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser resize audit");
    }
    complete_resize_result(root, &BrowserResizeResult::failed(request, code, message));
}

fn complete_resize_result(root: &std::path::Path, result: &BrowserResizeResult) {
    if let Err(error) = manifest::complete_resize_request_in(root, result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser resize result");
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelOptions, WorkspaceId, browser_actor};

    use super::*;
    use crate::app::test_support::test_app;

    fn agent_options() -> PanelOptions {
        let (command, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".to_string(), "exit 0".to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()])
        };
        PanelOptions {
            command: Some(command.to_string()),
            args,
            kind: PanelKind::Codex,
            ..PanelOptions::default()
        }
    }

    /// A browser panel without a driver, placed in `workspace` and listed in
    /// it, so preset-layout resizes reach it like a real browser panel.
    fn inert_browser(app: &mut HorizonApp, id: u64, workspace: WorkspaceId, remote: bool) -> (PanelId, String) {
        let browser = if remote {
            BrowserPanelState::inert_remote("ios_phone", "grid")
        } else {
            BrowserPanelState::inert()
        };
        // `assign_panel_to_workspace` keeps panels that already carry the
        // target id, so route through a throwaway workspace to join the list.
        let parking = app.board.create_workspace("parking");
        let panel = Panel::from_content(
            PanelId(id),
            parking,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(browser)),
        );
        let local_id = panel.local_id.clone();
        app.board.panels.push(panel);
        app.board.assign_panel_to_workspace(PanelId(id), workspace);
        (PanelId(id), local_id)
    }

    fn live_manifest_root(app: &HorizonApp) -> std::path::PathBuf {
        app.host_manifest_root().to_path_buf()
    }

    /// A live manifest under the app's test root: stamped for the workspace,
    /// owned by `actor`, the way the host publishes an owned panel.
    fn live_manifest(app: &HorizonApp, panel_local_id: &str, workspace: WorkspaceId, actor: &str) {
        let root = live_manifest_root(app);
        let workspace = browser_workspace(&app.board, workspace).expect("workspace");
        manifest::write_at(
            &manifest::manifest_path_for_root(&root, panel_local_id),
            &manifest::BrowserManifest {
                panel_local_id: panel_local_id.to_string(),
                host: Some(manifest::host_instance().to_string()),
                workspace: Some(workspace),
                owner: Some(manifest::ManifestOwner {
                    name: actor.to_string(),
                    tty: None,
                    updated_at: manifest::now_millis(),
                }),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("write live manifest");
    }

    fn published_result(app: &HorizonApp, actor: &str) -> Option<serde_json::Value> {
        let directory = live_manifest_root(app).join("runtime").join("browser-resize");
        let Ok(entries) = std::fs::read_dir(directory) else {
            return None;
        };
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".result.json"))
            .filter_map(|entry| {
                let raw = std::fs::read_to_string(entry.path()).ok()?;
                serde_json::from_str(&raw).ok()
            })
            .find(|value: &serde_json::Value| value["actor"].as_str() == Some(actor))
    }

    #[test]
    fn a_claimed_resize_applies_the_size_and_publishes_the_result() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent_id).expect("agent panel").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9001, alpha, false);
        live_manifest(&app, &browser_local_id, alpha, &actor);
        let request = BrowserResizeRequest::for_tests(&actor, &browser_local_id, 1920, 1080, i64::MAX);

        let changed = app.apply_browser_resize_request(&request, actor_panel);

        assert!(changed, "a size change marks the runtime dirty");
        let panel = app.board.panel(browser_id).expect("panel");
        assert!(
            approx_size(panel.layout.size, panel_size_for_viewport([1920, 1080])),
            "the panel is sized to render the requested viewport"
        );
        let manifest = manifest::read_at(&manifest::manifest_path_for_root(
            &live_manifest_root(&app),
            &browser_local_id,
        ))
        .expect("manifest");
        assert_eq!(manifest.viewport, Some([1920, 1080]));
        let result = published_result(&app, &actor).expect("result published");
        assert_eq!(result["outcome"]["status"], "ready");
        assert_eq!(result["outcome"]["width"].as_u64(), Some(1920));
        assert_eq!(result["outcome"]["height"].as_u64(), Some(1080));
    }

    #[test]
    fn an_out_of_workspace_resize_is_refused_with_the_result_published() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let beta = app.board.create_workspace("beta");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent_id).expect("agent panel").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (_, elsewhere_local_id) = inert_browser(&mut app, 9002, beta, false);
        let request = BrowserResizeRequest::for_tests(&actor, &elsewhere_local_id, 640, 480, i64::MAX);

        let changed = app.apply_browser_resize_request(&request, actor_panel);

        assert!(!changed, "a refusal leaves the board untouched");
        let result = published_result(&app, &actor).expect("refusal result published");
        assert_eq!(result["outcome"]["status"], "failed");
        assert_eq!(result["outcome"]["code"], "panel_outside_workspace");
    }

    #[test]
    fn refusals_keep_the_panel_and_name_the_reason() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent_id).expect("agent panel").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9003, alpha, false);
        let (remote_id, remote_local_id) = inert_browser(&mut app, 9004, alpha, true);
        live_manifest(&app, &browser_local_id, alpha, &actor);
        let agent_local_id = app.board.panel(agent_id).expect("agent panel").local_id.clone();

        let expired = BrowserResizeRequest::for_tests(&actor, &browser_local_id, 640, 480, 0);
        assert_eq!(
            app.resize_target(&expired, actor_panel, true, 1)
                .expect_err("expired")
                .code,
            "request_expired"
        );
        let missing = BrowserResizeRequest::for_tests(&actor, "browser-gone", 640, 480, i64::MAX);
        assert_eq!(
            app.resize_target(&missing, actor_panel, true, 0)
                .expect_err("missing")
                .code,
            "panel_not_in_host"
        );
        let not_browser = BrowserResizeRequest::for_tests(&actor, &agent_local_id, 640, 480, i64::MAX);
        assert_eq!(
            app.resize_target(&not_browser, actor_panel, true, 0)
                .expect_err("agent panel")
                .code,
            "not_browser_panel"
        );
        let remote = BrowserResizeRequest::for_tests(&actor, &remote_local_id, 640, 480, i64::MAX);
        assert_eq!(
            app.resize_target(&remote, actor_panel, true, 0)
                .expect_err("remote panel")
                .code,
            "remote_viewport_fixed",
            "a remote device keeps its fixed viewport"
        );
        let unowned = BrowserResizeRequest::for_tests(&actor, &browser_local_id, 640, 480, i64::MAX);
        assert_eq!(
            app.resize_target(&unowned, actor_panel, false, 0)
                .expect_err("ownership changed")
                .code,
            "ownership_changed"
        );
        let (panel_id, size) = app
            .resize_target(&unowned, actor_panel, true, 0)
            .expect("an owned browser panel in the agent's workspace may be resized");
        assert_eq!(panel_id, browser_id);
        assert!(
            approx_size(size, panel_size_for_viewport([640, 480])),
            "the resize target is the panel that renders the requested viewport"
        );
        assert!(app.board.panel(browser_id).is_some(), "every refusal leaves the panel");
        let _ = remote_id;
    }

    /// f32 layout sizes are assigned verbatim by the board; compare with the
    /// small tolerance the rest of the UI tests use.
    fn approx_size(actual: [f32; 2], expected: [f32; 2]) -> bool {
        actual.iter().zip(expected).all(|(a, b)| (a - b).abs() <= 0.01)
    }
}
