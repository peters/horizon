//! Native viewer host lifecycle. Standalone device input never enters this path.
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{
    PanelId, PanelKind, PanelOptions,
    browser::manifest::{
        self,
        device::{self, Operation, Outcome, PanelState, Request},
    },
};

use super::{
    HorizonApp,
    browser_requests::{ActorPanel, actor_panel},
};

impl HorizonApp {
    pub(super) fn poll_device_panel_requests(&mut self, ctx: &Context) -> bool {
        let now = Instant::now();
        if self
            .panel_render_caches
            .device_request_poll
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(250))
        {
            return false;
        }
        self.panel_render_caches.device_request_poll = Some(now);
        let requests = match device::claim(manifest::host_instance()) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(%error, "could not poll Device panel requests");
                return false;
            }
        };
        let changed = !requests.is_empty();
        for request in requests {
            let outcome = self.apply_device_request(&request, ctx);
            if let Err(error) = device::complete(&request, outcome) {
                tracing::warn!(%error, "could not publish Device panel result");
            }
        }
        changed
    }

    fn apply_device_request(&mut self, request: &Request, ctx: &Context) -> Outcome {
        if request.host_instance != manifest::host_instance() {
            return Outcome::failed("wrong_host", "Request belongs to another Horizon host");
        }
        if request.deadline_at_millis <= manifest::now_millis() {
            return Outcome::failed("request_expired", "Device panel request expired before dispatch");
        }
        let Some(actor) = actor_panel(&self.board, &request.actor) else {
            return Outcome::failed(
                "caller_unavailable",
                "The calling agent panel is no longer in this host",
            );
        };
        match &request.operation {
            Operation::Create { endpoint, identity } => {
                self.create_device_viewer(endpoint, identity.clone(), actor, &request.actor, ctx)
            }
            Operation::List => {
                let ids: Vec<_> = self
                    .board
                    .panels
                    .iter()
                    .filter(|panel| panel.workspace_id == actor.workspace_id && panel.device().is_some())
                    .map(|panel| panel.id)
                    .collect();
                Outcome::Panels {
                    panels: ids
                        .into_iter()
                        .filter_map(|id| self.device_observation(id, &request.actor))
                        .collect(),
                }
            }
            operation => self.change_device_viewer(operation, actor, &request.actor, ctx),
        }
    }

    fn create_device_viewer(
        &mut self,
        endpoint: &str,
        mut identity: Option<device::DeviceIdentity>,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        if let Some(identity) = &mut identity
            && let Err(error) = horizon_core::DevicePanelState::normalize_identity(identity)
        {
            return Outcome::failed("invalid_identity", &error.to_string());
        }
        let options = PanelOptions {
            kind: PanelKind::Device,
            device_identity: identity,
            command: Some(endpoint.into()),
            ..PanelOptions::default()
        };
        let focused = self.board.focused;
        let Ok(id) = self.board.create_panel(options, actor.workspace_id) else {
            return Outcome::failed(
                "invalid_endpoint",
                "Device viewer requires a numeric loopback address and nonzero port",
            );
        };
        #[cfg(feature = "cloud-workspaces")]
        self.cloud_attach_agent_child(actor.panel_id, id);
        // Creating a viewer must not steal keyboard input from the caller.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        let state = self.panel_render_caches.device_ui_state.entry(id).or_default();
        state.owner = Some(owner.into());
        if let Some(device) = self.board.panel(id).and_then(horizon_core::Panel::device) {
            state.reconnect(ctx, device);
        }
        self.mark_runtime_dirty();
        self.single_device_observation(id, owner)
    }

    fn change_device_viewer(
        &mut self,
        operation: &Operation,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        let (Operation::Inspect { panel_id }
        | Operation::Visibility { panel_id, .. }
        | Operation::Reveal { panel_id }
        | Operation::Reconnect { panel_id }
        | Operation::Close { panel_id }) = operation
        else {
            return Outcome::failed("invalid_operation", "Expected a panel operation");
        };
        let Some(id) = self.board.panel_id_by_local_id(panel_id) else {
            return Outcome::failed("panel_unavailable", "Device panel is unavailable in this workspace");
        };
        let Some(panel) = self
            .board
            .panel(id)
            .filter(|panel| panel.workspace_id == actor.workspace_id)
        else {
            return Outcome::failed("panel_unavailable", "Device panel is unavailable in this workspace");
        };
        let Some(device) = panel.device().cloned() else {
            return Outcome::failed("not_device_panel", "Target is not a native Device viewer");
        };
        let state = self.panel_render_caches.device_ui_state.entry(id).or_default();
        if matches!(operation, Operation::Inspect { .. }) {
            return self.single_device_observation(id, owner);
        }
        let can_acquire = state.owner.is_none() && matches!(operation, Operation::Reconnect { .. });
        if state.owner.as_deref() != Some(owner) && !can_acquire {
            return Outcome::failed(
                "not_owner",
                "Only the owning agent may mutate this Device panel; reconnect explicitly to acquire an unowned viewer",
            );
        }
        match operation {
            Operation::Reconnect { .. } => {
                state.owner = Some(owner.into());
                state.reconnect(ctx, &device);
            }
            Operation::Visibility { visible, .. } => {
                self.board.set_panel_visible(id, *visible);
                if !visible && self.fullscreen_panel == Some(id) {
                    self.fullscreen_panel = None;
                }
                if !visible && self.board.focused.is_none() {
                    self.board.focus(actor.panel_id);
                }
                self.mark_runtime_dirty();
            }
            Operation::Reveal { .. } => {
                self.reveal_device_viewer(ctx, id, actor);
            }
            Operation::Close { panel_id } => {
                if self.fullscreen_panel == Some(id) {
                    self.fullscreen_panel = None;
                }
                let focused = self.board.focused == Some(id);
                let _ = self.close_panel_returning_teardown(id);
                // Dropping DeviceUiState joins the worker and releases its socket.
                self.panel_render_caches.device_ui_state.remove(&id);
                if focused {
                    self.board.focus(actor.panel_id);
                }
                self.mark_runtime_dirty();
                return Outcome::Closed {
                    panel_id: panel_id.clone(),
                };
            }
            _ => {}
        }
        self.single_device_observation(id, owner)
    }

    fn single_device_observation(&mut self, id: PanelId, owner: &str) -> Outcome {
        self.device_observation(id, owner).map_or_else(
            || Outcome::failed("panel_unavailable", "Device panel closed"),
            |panel| Outcome::Panels { panels: vec![panel] },
        )
    }

    fn reveal_device_viewer(&mut self, ctx: &Context, id: PanelId, actor: ActorPanel) {
        let focused = self.board.focused;
        let active_workspace = self.board.active_workspace;
        self.board.set_panel_visible(id, true);
        if let Some(workspace) = self.board.workspace_mut(actor.workspace_id) {
            workspace.collapsed = false;
        }
        #[cfg(feature = "cloud-workspaces")]
        if let Some(local) = self.board.panel(id).map(|panel| panel.local_id.clone()) {
            let mut expanded_cloud = false;
            for group in &mut self.cloud_prototype.groups.0 {
                if group.panels.contains(&local) {
                    group.set_collapsed(&mut self.board, false);
                    expanded_cloud = true;
                }
            }
            if expanded_cloud {
                self.board.cloud_groups = self.cloud_prototype.groups.clone();
            }
        }
        let local = self
            .board
            .workspace(actor.workspace_id)
            .map(|workspace| workspace.local_id.clone());
        if let Some(state) = local.and_then(|local| self.detached_workspaces.get_mut(&local)) {
            // The detached viewport applies this using its own geometry next frame.
            // Never send an OS Focus command from an automated reveal.
            state.pending_device_reveal = Some(id);
        } else {
            if self.fullscreen_panel.is_some_and(|current| current != id) {
                self.fullscreen_panel = None;
            }
            self.reveal_new_panel(ctx, actor.workspace_id, id);
        }
        self.board.focused = focused;
        self.board.active_workspace = active_workspace;
        self.mark_runtime_dirty();
        ctx.request_repaint();
    }

    pub(super) fn apply_pending_device_reveal(&mut self, local: &str, canvas: egui::Rect) {
        let pending = self
            .detached_workspaces
            .get_mut(local)
            .and_then(|state| state.pending_device_reveal.take());
        let Some(id) = pending else { return };
        let valid = self.board.panel(id).is_some_and(|panel| {
            panel.device().is_some()
                && self
                    .board
                    .workspace(panel.workspace_id)
                    .is_some_and(|workspace| workspace.local_id == local)
        });
        if valid {
            let focused = self.board.focused;
            let active_workspace = self.board.active_workspace;
            self.reveal_panel_in_rect(id, canvas);
            self.board.focused = focused;
            self.board.active_workspace = active_workspace;
        }
    }

    fn device_observation(&mut self, id: PanelId, owner: &str) -> Option<PanelState> {
        let panel = self.board.panel(id)?;
        Some(
            self.panel_render_caches
                .device_ui_state
                .entry(id)
                .or_default()
                .observation(panel.local_id.clone(), panel.device()?, panel.visible, owner),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
    use horizon_core::{RuntimeState, StartupDecision};

    fn app() -> (tempfile::TempDir, Context, HorizonApp) {
        let runtime = RuntimeState {
            workspaces: vec![
                editor_workspace_state("first", [0.0, 0.0]),
                editor_workspace_state("second", [900.0, 0.0]),
            ],
            ..RuntimeState::default()
        };
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime),
        });
        // No live agent process is needed to exercise host authorization.
        app.board.panels[0].kind = PanelKind::Claude;
        app.board.panels[1].kind = PanelKind::Claude;
        (temp, ctx, app)
    }

    fn request(app: &HorizonApp, operation: Operation) -> Request {
        Request {
            request_id: "test".into(),
            actor: format!("horizon:{}", app.board.panels[0].local_id),
            host_instance: manifest::host_instance().into(),
            deadline_at_millis: manifest::now_millis() + 10_000,
            operation,
        }
    }

    fn one(outcome: Outcome) -> PanelState {
        let Outcome::Panels { mut panels } = outcome else {
            panic!("expected panels: {outcome:?}")
        };
        assert_eq!(panels.len(), 1);
        panels.remove(0)
    }

    #[test]
    fn lifecycle_preserves_other_panels_and_never_calls_creation_live_image_proof() {
        let (_temp, ctx, mut app) = app();
        let original_ids: Vec<_> = app.board.panels.iter().map(|p| p.local_id.clone()).collect();
        let create = request(
            &app,
            Operation::Create {
                identity: None,
                endpoint: "127.0.0.1:5900".into(),
            },
        );
        let panel = one(app.apply_device_request(&create, &ctx));
        assert!(panel.visible && panel.owned_by_caller);
        assert!(!panel.image.image_received && !panel.image.image_displayed);
        let id = panel.panel_id;
        for visible in [false, true] {
            let request = request(
                &app,
                Operation::Visibility {
                    panel_id: id.clone(),
                    visible,
                },
            );
            assert_eq!(one(app.apply_device_request(&request, &ctx)).visible, visible);
        }
        let close = request(&app, Operation::Close { panel_id: id.clone() });
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Closed { .. }));
        assert_eq!(
            app.board.panels.iter().map(|p| p.local_id.clone()).collect::<Vec<_>>(),
            original_ids
        );
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { .. }));
    }

    #[test]
    fn host_workspace_owner_expiry_and_endpoint_checks_precede_mutation() {
        let (_temp, ctx, mut app) = app();
        for endpoint in ["localhost:5900", "192.0.2.1:5900", "127.0.0.1:0"] {
            let create = request(
                &app,
                Operation::Create {
                    identity: None,
                    endpoint: endpoint.into(),
                },
            );
            assert!(matches!(
                app.apply_device_request(&create, &ctx),
                Outcome::Failed { .. }
            ));
        }
        let create = request(
            &app,
            Operation::Create {
                identity: None,
                endpoint: "127.0.0.1:5900".into(),
            },
        );
        let panel = one(app.apply_device_request(&create, &ctx));
        let mut close = request(
            &app,
            Operation::Close {
                panel_id: panel.panel_id.clone(),
            },
        );
        close.host_instance = "another-host".into();
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { .. }));
        close.host_instance = manifest::host_instance().into();
        close.deadline_at_millis = 0;
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { .. }));
        close.deadline_at_millis = manifest::now_millis() + 10_000;
        close.actor = format!("horizon:{}", app.board.panels[1].local_id);
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { .. }));
        // Move the other agent into the same workspace: discovery succeeds, ownership still refuses close.
        app.board.panels[1].workspace_id = app.board.panels[0].workspace_id;
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { code, .. } if code == "not_owner"));
        close.operation = Operation::Inspect {
            panel_id: panel.panel_id,
        };
        assert!(!one(app.apply_device_request(&close, &ctx)).owned_by_caller);
    }
    #[test]
    fn reveal_preserves_focus_and_connection_while_restoring_visibility() {
        let (_temp, ctx, mut app) = app();
        let create = request(
            &app,
            Operation::Create {
                identity: None,
                endpoint: "127.0.0.1:5900".into(),
            },
        );
        let initial = one(app.apply_device_request(&create, &ctx));
        let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
        let caller = app.board.panels[0].id;
        let workspace = app.board.panels[0].workspace_id;
        app.board.set_panel_visible(id, false);
        app.board.panel_mut(id).unwrap().layout.position = [5000.0, 5000.0];
        app.board.workspace_mut(workspace).unwrap().collapsed = true;
        app.board.focus(caller);
        app.fullscreen_panel = Some(caller);
        let before = app.canvas_view;
        let reveal = request(
            &app,
            Operation::Reveal {
                panel_id: initial.panel_id,
            },
        );
        let observed = one(app.apply_device_request(&reveal, &ctx));
        assert!(observed.visible);
        assert!(!app.board.workspace(workspace).unwrap().collapsed);
        assert_eq!(app.board.focused, Some(caller));
        assert!(app.fullscreen_panel.is_none());
        assert_ne!(app.canvas_view, before);
        assert_eq!(
            observed.diagnostics.unwrap().connection_generation,
            initial.diagnostics.unwrap().connection_generation
        );
        assert!(
            !observed.image.image_displayed,
            "reveal must not fabricate a painted image"
        );
    }

    #[test]
    fn detached_reveal_queues_its_own_canvas_without_focusing_the_window() {
        let (_temp, ctx, mut app) = app();
        let create = request(
            &app,
            Operation::Create {
                identity: None,
                endpoint: "127.0.0.1:5900".into(),
            },
        );
        let initial = one(app.apply_device_request(&create, &ctx));
        let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
        let workspace = app.board.panels[0].workspace_id;
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        app.detach_workspace(workspace);
        app.board.panel_mut(id).unwrap().layout.position = [5000.0, 5000.0];
        let caller = app.board.panels[1].id;
        app.board.focus(caller);
        let root_view = app.canvas_view;
        let active_workspace = app.board.active_workspace;
        let reveal = request(
            &app,
            Operation::Reveal {
                panel_id: initial.panel_id,
            },
        );
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = one(app.apply_device_request(&reveal, ui.ctx()));
        });
        output.textures_delta.clear();
        assert!(output.viewport_output.values().all(|viewport| {
            !viewport
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus))
        }));
        assert_eq!(app.canvas_view, root_view);
        assert_eq!(app.board.focused, Some(caller));
        assert_eq!(app.board.active_workspace, active_workspace);
        assert_eq!(app.detached_workspaces[&local].pending_device_reveal, Some(id));
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        app.canvas_view = app.detached_workspaces[&local].canvas_view;
        app.apply_pending_device_reveal(&local, canvas);
        let panel = app.board.panel(id).unwrap();
        let rect = egui::Rect::from_min_size(
            egui::Pos2::from(panel.layout.position),
            egui::Vec2::from(panel.layout.size),
        );
        let transform = crate::app::view::canvas_scene_transform(canvas, app.canvas_view);
        assert!(canvas.intersects(transform * rect));
        assert!(app.detached_workspaces[&local].pending_device_reveal.is_none());
        assert_eq!(app.board.focused, Some(caller));
        assert_eq!(app.board.active_workspace, active_workspace);
    }

    #[test]
    fn restored_viewer_requires_explicit_reconnect_to_acquire_ownership() {
        let (_temp, ctx, mut app) = app();
        let saved = horizon_core::PanelState {
            kind: PanelKind::Device,
            command: Some("127.0.0.1:1".into()),
            ..Default::default()
        };
        let id = app
            .board
            .create_panel(
                saved.to_panel_options(&horizon_core::browser::BrowserConfig::default()),
                app.board.panels[0].workspace_id,
            )
            .unwrap();
        let panel = app.board.panel(id).unwrap();
        assert!(!panel.device().unwrap().connect_on_start);
        let local_id = panel.local_id.clone();
        let inspect = request(
            &app,
            Operation::Inspect {
                panel_id: local_id.clone(),
            },
        );
        let stopped = one(app.apply_device_request(&inspect, &ctx));
        assert!(!stopped.owned_by_caller && !stopped.image.image_received && !stopped.image.image_displayed);
        assert_eq!(stopped.connection, device::Connection::Stopped);
        let close = request(
            &app,
            Operation::Close {
                panel_id: local_id.clone(),
            },
        );
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Failed { code, .. } if code == "not_owner"));
        let reconnect = request(&app, Operation::Reconnect { panel_id: local_id });
        let acquired = one(app.apply_device_request(&reconnect, &ctx));
        assert!(acquired.owned_by_caller);
        assert!(!acquired.image.image_received && !acquired.image.image_displayed);
        assert!(matches!(app.apply_device_request(&close, &ctx), Outcome::Closed { .. }));
    }
    #[test]
    fn create_list_and_inspect_share_normalized_supplied_identity() {
        let (_temp, ctx, mut app) = app();
        let identity = device::DeviceIdentity {
            machine_name: Some("  Lab workstation  ".into()),
            hostname: Some("lab-host".into()),
            ip_addresses: vec!["192.0.2.10".parse().unwrap()],
            tailscale_name: Some("lab-host.example.ts.net".into()),
        };
        let create = request(
            &app,
            Operation::Create {
                endpoint: "127.0.0.1:5900".into(),
                identity: Some(identity),
            },
        );
        let created = one(app.apply_device_request(&create, &ctx));
        assert_eq!(
            created.identity.as_ref().unwrap().machine_name.as_deref(),
            Some("Lab workstation")
        );
        for operation in [
            Operation::List,
            Operation::Inspect {
                panel_id: created.panel_id,
            },
        ] {
            let request = request(&app, operation);
            let observed = one(app.apply_device_request(&request, &ctx));
            assert_eq!(observed.identity, created.identity);
        }
        let count = app.board.panels.len();
        let invalid = request(
            &app,
            Operation::Create {
                endpoint: "127.0.0.1:5900".into(),
                identity: Some(device::DeviceIdentity {
                    machine_name: Some("a".repeat(257)),
                    ..Default::default()
                }),
            },
        );
        assert!(
            matches!(app.apply_device_request(&invalid, &ctx), Outcome::Failed { code, .. } if code == "invalid_identity")
        );
        assert_eq!(app.board.panels.len(), count);
    }

    #[cfg(feature = "cloud-workspaces")]
    #[test]
    fn reveal_saves_cloud_expansion_immediately_without_erasing_unprepared_groups() {
        use horizon_core::{CanvasViewState, WindowConfig, cloud_panel::CloudGroup};
        for belongs in [true, false] {
            let (_temp, ctx, mut app) = app();
            let create = request(
                &app,
                Operation::Create {
                    endpoint: "127.0.0.1:5900".into(),
                    identity: None,
                },
            );
            let viewer = one(app.apply_device_request(&create, &ctx));
            let id = app.board.panel_id_by_local_id(&viewer.panel_id).unwrap();
            let workspace = app.board.panels[0].workspace_id;
            let local = app.board.workspace(workspace).unwrap().local_id.clone();
            let mut group = CloudGroup::new(1, "Cloud".into(), local, std::path::PathBuf::new(), [0.0, 0.0]);
            if belongs {
                group.panels.push(viewer.panel_id.clone());
                group.set_collapsed(&mut app.board, true);
                app.cloud_prototype.groups.0.push(group.clone());
            }
            app.board.cloud_groups.0.push(group);
            let reveal = request(
                &app,
                Operation::Reveal {
                    panel_id: viewer.panel_id,
                },
            );
            assert!(one(app.apply_device_request(&reveal, &ctx)).visible);
            let saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
            assert_eq!(
                saved.cloud_groups.0.len(),
                1,
                "unprepared saved groups must survive unrelated Reveal"
            );
            assert!(
                !saved.cloud_groups.0[0].collapsed,
                "expansion must persist before another render frame"
            );
            if belongs {
                assert!(
                    saved.cloud_groups.0[0]
                        .panels
                        .contains(&app.board.panel(id).unwrap().local_id)
                );
            }
        }
    }
}
