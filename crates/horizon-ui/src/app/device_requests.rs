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
            Operation::Create { endpoint } => self.create_device_viewer(endpoint, actor, &request.actor, ctx),
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

    fn create_device_viewer(&mut self, endpoint: &str, actor: ActorPanel, owner: &str, ctx: &Context) -> Outcome {
        let options = PanelOptions {
            kind: PanelKind::Device,
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
}
