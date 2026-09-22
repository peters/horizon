//! A cloud view reuses the canvas and panels without changing their saved visibility.
use egui::{Context, Id, Order, RichText, Vec2, ViewportCommand};
use horizon_core::{CanvasViewState, PanelId};

use super::HorizonApp;
use crate::theme;

pub(in crate::app) struct CloudFullscreen {
    pub id: u32,
    pub previous_view: CanvasViewState,
    previous_focus: Option<PanelId>,
    previous_window_fullscreen: bool,
    previous_window_size: Vec2,
}

impl HorizonApp {
    pub(in crate::app) fn exit_cloud_for_device_reveal(
        &mut self,
        ctx: &Context,
    ) -> Option<crate::app::device_requests::WindowRestore> {
        let expected =
            self.cloud_prototype
                .fullscreen
                .as_ref()
                .map(|view| crate::app::device_requests::WindowRestore {
                    fullscreen: view.previous_window_fullscreen,
                    size: view.previous_window_size,
                });
        self.exit_cloud_fullscreen(ctx);
        expected
    }

    pub(in crate::app) fn toggle_cloud_fullscreen(&mut self, ctx: &Context, id: u32) {
        if self.cloud_prototype.fullscreen.is_some() {
            self.exit_cloud_fullscreen(ctx);
            return;
        }
        let Some(group) = self.cloud_prototype.groups.0.iter_mut().find(|g| g.issue == id) else {
            return;
        };
        group.set_collapsed(&mut self.board, false);
        let previous_focus = self.board.focused;
        if self
            .board
            .focused
            .and_then(|id| self.board.panel(id))
            .is_some_and(|p| !group.panels.contains(&p.local_id))
        {
            self.board.focused = None;
        }
        self.cloud_prototype.fullscreen = Some(CloudFullscreen {
            id,
            previous_view: self.canvas_view,
            previous_focus,
            previous_window_fullscreen: ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)),
            previous_window_size: ctx.content_rect().size(),
        });
        self.pan_target = None;
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
        ctx.request_repaint();
    }

    pub(in crate::app) fn exit_cloud_fullscreen(&mut self, ctx: &Context) {
        if let Some(view) = self.cloud_prototype.fullscreen.take() {
            self.canvas_view = view.previous_view;
            self.board.focused = view
                .previous_focus
                .filter(|id| self.board.panel(*id).is_some_and(|p| p.visible));
            self.pan_target = None;
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(view.previous_window_fullscreen));
            ctx.request_repaint();
        }
    }

    pub(in crate::app) fn handle_cloud_fullscreen_exit(&mut self, ctx: &Context) {
        let binding = horizon_core::ShortcutBinding::new(
            horizon_core::ShortcutModifiers::NONE,
            horizon_core::ShortcutKey::Escape,
        );
        if self.cloud_prototype.fullscreen.is_some()
            && self.fullscreen_panel.is_none()
            && self.command_palette.is_none()
            && !self.speech_escape_cancelled
            && ctx.input(|i| crate::app::shortcuts::shortcut_pressed(i, binding))
        {
            self.consume_navigation_key(ctx, binding);
            self.exit_cloud_fullscreen(ctx);
        }
    }

    pub(in crate::app) fn fullscreen_cloud_target(&self) -> Option<(horizon_core::WorkspaceId, [f32; 2])> {
        let view = self.cloud_prototype.fullscreen.as_ref()?;
        let group = self.cloud_prototype.groups.0.iter().find(|g| g.issue == view.id)?;
        let workspace = self.board.workspace_id_by_local_id(&group.workspace)?;
        Some((
            workspace,
            [
                group.position[0] + horizon_core::cloud_panel::PAD,
                group.position[1] + horizon_core::cloud_panel::HEADER,
            ],
        ))
    }

    pub(in crate::app) fn cloud_panel_is_in_view(&self, local_id: &str) -> bool {
        self.cloud_prototype.fullscreen.as_ref().is_none_or(|view| {
            self.cloud_prototype
                .groups
                .0
                .iter()
                .any(|g| g.issue == view.id && g.panels.iter().any(|id| id == local_id))
        })
    }

    pub(in crate::app) fn render_fullscreen_cloud(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(view) = self.cloud_prototype.fullscreen.as_ref() else {
            return false;
        };
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|g| g.issue == view.id) else {
            self.exit_cloud_fullscreen(ui.ctx());
            return false;
        };
        let title = group.title.clone();
        let (min, max) = group.overview_bounds();
        self.cloud_fit(ui.ctx(), min, max);
        let events = ui.ctx().input(|input| input.events.clone());
        self.terminal_keyboard_events = self.terminal_events_for_viewport(ui.ctx(), &events);
        self.canvas_pan_input_claimed = false;
        self.render_canvas(ui);
        self.render_cloud_frames(ui.ctx());
        self.handle_canvas_double_click(ui);
        self.render_panels(ui);
        self.render_cloud_ownership(ui.ctx());
        self.render_preset_picker(ui);
        if std::env::var_os("HORIZON_CLOUD_MOCK_DIR").is_some() {
            self.render_cloud_runtimes(ui.ctx());
        } else {
            self.render_production_runtimes(ui.ctx());
        }
        let mut exit = false;
        egui::Area::new(Id::new("cloud-fullscreen-navigation"))
            .order(Order::Tooltip)
            .fixed_pos(egui::pos2(24.0, 18.0))
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    exit = ui.button("←  All clouds").clicked();
                    ui.add_space(16.0);
                    ui.label(RichText::new(title).size(18.0).strong());
                    ui.add_space(12.0);
                    ui.label(RichText::new("Full screen  ·  Esc to return").color(theme::FG_DIM()));
                    ui.allocate_space(Vec2::new(0.0, 28.0));
                });
            });
        if exit {
            self.exit_cloud_fullscreen(ui.ctx());
        }
        if let Some(error) = &self.cloud_prototype.error {
            egui::Area::new(Id::new("cloud-fullscreen-error"))
                .order(Order::Tooltip)
                .fixed_pos(egui::pos2(24.0, 64.0))
                .show(ui.ctx(), |ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                });
        }
        true
    }
}

#[cfg(all(test, unix))]
mod navigation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_panel::{CloudGroup, CloudGroups};
    use horizon_core::{PanelKind, PanelOptions};

    #[test]
    fn cloud_fullscreen_persists_overview_and_releases_hidden_overlay_regions() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let workspace = app.board.create_workspace("fixture");
        app.cloud_prototype.groups.0.push(CloudGroup::new(
            1,
            "Fixture".into(),
            app.board.workspace(workspace).unwrap().local_id.clone(),
            temp.path().into(),
            [0.0, 0.0],
        ));
        let session = app
            .session_store
            .create_session_from_runtime(horizon_core::RuntimeState::default())
            .unwrap();
        app.active_session = Some(crate::app::ActiveSession {
            session_id: session.session_id,
            persistent: true,
            lease: None,
            last_lease_refresh: None,
        });
        app.root_viewport_stabilizer = None;
        app.pending_startup_runtime_state = None;
        let overview = CanvasViewState::new([140.0, 180.0], 0.7);
        app.canvas_view = overview;
        app.toggle_cloud_fullscreen(&ctx, 1);
        app.canvas_view = CanvasViewState::new([40.0, 80.0], 1.4);
        assert!(app.auto_save_runtime_state());
        let saved: horizon_core::RuntimeState =
            serde_yaml::from_str(&std::fs::read_to_string(session.runtime_state_path).unwrap()).unwrap();
        assert_eq!(saved.canvas_view_or_default(), overview);
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                    ..Default::default()
                },
                |_ui| {
                    let excluded = app.overlay_exclusion_zones(&ctx);
                    assert!(excluded.contains(egui::pos2(100.0, 30.0)));
                    assert!(
                        !excluded.contains(egui::pos2(100.0, 300.0)),
                        "hidden sidebar must not swallow cloud input"
                    );
                    assert!(
                        !excluded.contains(egui::pos2(1350.0, 850.0)),
                        "hidden minimap must not swallow cloud input"
                    );
                },
            )
            .discard_textures();
    }

    #[test]
    fn cloud_fullscreen_refreshes_keyboard_events_each_frame() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let ws = app.board.create_workspace_at("desk", [0.0, 0.0]);
        let local = app.board.workspace(ws).unwrap().local_id.clone();
        app.cloud_prototype.groups = CloudGroups(vec![CloudGroup::new(
            101,
            "Cloud".into(),
            local,
            temp.path().into(),
            [0.0, 0.0],
        )]);
        app.toggle_cloud_fullscreen(&ctx, 101);
        let input = egui::RawInput {
            events: vec![egui::Event::Text("remote input".into())],
            ..Default::default()
        };
        let _ = ctx
            .run_ui(input, |ui| {
                app.render_fullscreen_cloud(ui);
            })
            .discard_textures();
        assert!(
            app.terminal_keyboard_events
                .iter()
                .any(|event| matches!(&event.event, egui::Event::Text(value) if value == "remote input"))
        );
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                app.render_fullscreen_cloud(ui);
            })
            .discard_textures();
        assert!(app.terminal_keyboard_events.is_empty());
    }

    #[test]
    fn fullscreen_scopes_rendering_without_hiding_sessions_and_restores_view() {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        let ws = app.board.create_workspace_at("desk", [0.0, 0.0]);
        let local = app.board.workspace(ws).unwrap().local_id.clone();
        let mut groups = Vec::new();
        let mut ids = Vec::new();
        for id in 1..=2 {
            let panel = app
                .board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        ..PanelOptions::default()
                    },
                    ws,
                )
                .unwrap();
            let mut group = CloudGroup::new(id, format!("Cloud {id}"), local.clone(), temp.path().into(), [0.0, 0.0]);
            group.attach(&mut app.board, panel);
            groups.push(group);
            ids.push(panel);
        }
        app.cloud_prototype.groups = CloudGroups(groups);
        app.board.focused = Some(ids[1]);
        let before = app.canvas_view;
        app.toggle_cloud_fullscreen(&ctx, 1);
        assert!(app.board.panels.iter().all(|p| p.visible));
        assert!(app.cloud_panel_is_in_view(&app.board.panel(ids[0]).unwrap().local_id));
        assert!(!app.cloud_panel_is_in_view(&app.board.panel(ids[1]).unwrap().local_id));
        assert!(app.board.focused.is_none());
        app.canvas_view = CanvasViewState::new([400.0, 200.0], 0.5);
        app.exit_cloud_fullscreen(&ctx);
        assert_eq!(app.canvas_view, before);
        assert_eq!(app.board.focused, Some(ids[1]));
        assert!(
            app.board
                .panels
                .iter()
                .all(|p| p.visible && app.cloud_panel_is_in_view(&p.local_id))
        );
        app.toggle_cloud_fullscreen(&ctx, 1);
        let (target_workspace, target_position) = app.fullscreen_cloud_target().unwrap();
        assert_eq!(target_workspace, ws);
        assert_eq!(
            app.cloud_prototype.groups.at_position(&app.board, ws, target_position),
            Some(0)
        );
        app.add_panel_to_workspace(
            &ctx,
            ws,
            horizon_core::PresetConfig {
                name: "Usage".into(),
                alias: None,
                kind: PanelKind::Usage,
                command: None,
                args: Vec::new(),
                resume: horizon_core::PanelResume::Fresh,
                ssh_connection: None,
            },
            None,
        );
        assert!(app.dir_picker.is_none());
        assert_eq!(app.cloud_prototype.groups.0[0].panels.len(), 2);
        assert_eq!(app.cloud_prototype.groups.0[1].panels.len(), 1);
        let members: Vec<_> = app
            .board
            .panels
            .iter()
            .filter(|panel| app.cloud_prototype.groups.0[0].panels.contains(&panel.local_id))
            .collect();
        assert!(members[1].layout.position[0] >= members[0].layout.position[0] + members[0].layout.size[0]);
        assert!(app.cloud_prototype.fullscreen.is_some());
        app.reveal_selected_panel(&ctx, ids[1]);
        assert!(app.cloud_prototype.fullscreen.is_none());
        assert_eq!(app.board.focused, Some(ids[1]));
    }
}
