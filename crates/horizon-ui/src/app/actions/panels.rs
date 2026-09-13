use crate::app::HorizonApp;
use crate::dir_picker::{DirPicker, DirPickerPurpose};
use horizon_core::{PanelId, PanelOptions, PanelResume, PanelTranscript, PresetConfig, WorkspaceId};

use super::{add_panel_position, inherit_workspace_cwd, workspace_cwd};

/// Newly added agent panels always start a new session. `resume: last` on a
/// preset only governs how existing panels reconnect when they are restored.
/// Continue-style agents (`KiloCode`) keep `last` on the panel because their
/// reconnect flag is applied only on restore, not on the initial launch.
fn normalize_new_panel_resume(options: &mut PanelOptions) {
    if options.kind.supports_session_binding() && matches!(options.resume, PanelResume::Last) {
        options.resume = PanelResume::Fresh;
    }
}

impl HorizonApp {
    pub(in crate::app) fn create_panel(&mut self, ctx: &egui::Context) {
        let workspace_id = self.ensure_workspace_visible(ctx);
        match self.create_panel_with_options(PanelOptions::default(), workspace_id) {
            Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
            Err(error) => tracing::error!("failed to create panel: {error}"),
        }
    }

    pub(in crate::app) fn create_panel_with_options(
        &mut self,
        mut options: PanelOptions,
        workspace_id: WorkspaceId,
    ) -> horizon_core::Result<PanelId> {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        inherit_workspace_cwd(&mut options, workspace_cwd.as_ref());
        normalize_new_panel_resume(&mut options);
        options.transcript_root.clone_from(&self.transcript_root);
        self.board.create_panel(options, workspace_id)
    }

    /// Reveal a panel the same way a sidebar panel-row click does: detached
    /// workspaces get their OS window focused, attached canvases pan/zoom so
    /// the panel is visible.
    pub(in crate::app) fn reveal_selected_panel(&mut self, ctx: &egui::Context, panel_id: PanelId) {
        match self.board.panel_workspace_id(panel_id) {
            Some(workspace_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
            None => self.board.focus(panel_id),
        }
    }

    /// Reveal a newly created panel the same way the sidebar reveals a
    /// selected panel: detached workspaces get their OS window focused, the
    /// panel re-focused so the helper stays self-contained, and attached
    /// canvases pan/zoom to the panel.
    pub(in crate::app) fn reveal_new_panel(
        &mut self,
        ctx: &egui::Context,
        workspace_id: WorkspaceId,
        panel_id: PanelId,
    ) {
        if self.focus_workspace_window(ctx, workspace_id) {
            self.board.focus(panel_id);
            return;
        }
        self.reveal_panel_visible(ctx, panel_id);
    }

    pub(in crate::app) fn close_panel(&mut self, panel_id: PanelId) {
        let transcript = self
            .board
            .panel(panel_id)
            .and_then(|panel| PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id));
        self.board.close_panel(panel_id);
        self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
        self.panel_render_caches.editor_preview_cache.remove(&panel_id);
        self.panel_render_caches.browser_ui_state.remove(&panel_id);
        if let Some(transcript) = transcript
            && let Err(error) = transcript.delete_all()
        {
            tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
        }
    }

    pub(in crate::app) fn close_workspace_panels(&mut self, workspace_id: WorkspaceId) {
        let panels_to_close: Vec<_> = self
            .board
            .workspace(workspace_id)
            .map(|workspace| {
                workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| {
                        self.board.panel(*panel_id).map(|panel| {
                            (
                                *panel_id,
                                PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if panels_to_close.is_empty() {
            self.board.close_panels_in_workspace(workspace_id);
            return;
        }

        let closed_panel_ids = self.board.close_panels_in_workspace(workspace_id);
        for panel_id in &closed_panel_ids {
            self.panel_screen_rects.remove(panel_id);
            self.terminal_body_screen_rects.remove(panel_id);
            self.panel_render_caches.terminal_grid_cache.remove(panel_id);
            self.panel_render_caches.editor_preview_cache.remove(panel_id);
            self.panel_render_caches.browser_ui_state.remove(panel_id);
        }

        if self
            .renaming_panel
            .is_some_and(|panel_id| closed_panel_ids.contains(&panel_id))
        {
            self.clear_panel_rename();
        }

        for (panel_id, transcript) in panels_to_close {
            if let Some(transcript) = transcript
                && let Err(error) = transcript.delete_all()
            {
                tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
            }
        }
    }

    pub(in crate::app) fn clear_workspace_rename(&mut self) {
        self.renaming_workspace = None;
        self.rename_buffer.clear();
    }

    pub(in crate::app) fn clear_panel_rename(&mut self) {
        self.renaming_panel = None;
        self.panel_rename_buffer.clear();
    }

    pub(in crate::app) fn add_panel_to_workspace(
        &mut self,
        ctx: &egui::Context,
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        if workspace_cwd(&self.board, workspace_id).is_some() || !preset.requires_workspace_cwd() {
            let mut options = preset.to_panel_options(&self.template_config.browser);
            options.position = add_panel_position(&self.board, workspace_id, canvas_pos);
            match self.create_panel_with_options(options, workspace_id) {
                Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
                Err(error) => tracing::error!("failed to create panel: {error}"),
            }
            self.mark_runtime_dirty();
        } else {
            self.open_panel_dir_picker(workspace_id, preset, canvas_pos);
        }
    }

    pub(in crate::app) fn open_panel_dir_picker(
        &mut self,
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        self.dir_picker = Some(DirPicker::with_seed(
            DirPickerPurpose::AddPanel {
                workspace_id,
                preset,
                canvas_pos,
            },
            workspace_cwd.as_deref(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelOptions, PanelResume, PresetConfig, RuntimeState, StartupDecision};

    use super::normalize_new_panel_resume;

    #[test]
    fn new_agent_panels_always_start_fresh_sessions() {
        for kind in [
            PanelKind::Claude,
            PanelKind::Codex,
            PanelKind::OpenCode,
            PanelKind::Pi,
            PanelKind::Grok,
        ] {
            let mut options = PanelOptions {
                kind,
                resume: PanelResume::Last,
                ..PanelOptions::default()
            };

            normalize_new_panel_resume(&mut options);

            assert_eq!(options.resume, PanelResume::Fresh, "kind {kind:?}");
        }
    }

    #[test]
    fn continue_style_agents_keep_last_resume() {
        let mut options = PanelOptions {
            kind: PanelKind::KiloCode,
            resume: PanelResume::Last,
            ..PanelOptions::default()
        };

        normalize_new_panel_resume(&mut options);

        assert_eq!(options.resume, PanelResume::Last);
    }

    #[test]
    fn explicitly_requested_sessions_are_preserved() {
        let mut options = PanelOptions {
            kind: PanelKind::Claude,
            resume: PanelResume::Session {
                session_id: "session-1".to_string(),
            },
            ..PanelOptions::default()
        };

        normalize_new_panel_resume(&mut options);

        assert_eq!(
            options.resume,
            PanelResume::Session {
                session_id: "session-1".to_string(),
            }
        );
    }

    fn shell_preset() -> PresetConfig {
        PresetConfig {
            name: "shell".to_string(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn new_panel_reveals_it_when_the_view_is_panned_away() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        // Establish the viewport so canvas math matches the assertion below.
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));

        let workspace_id = app.board.create_workspace("far");
        {
            let workspace = app.board.workspace_mut(workspace_id).expect("workspace");
            workspace.position = [6000.0, 4000.0];
            workspace.cwd = Some(std::path::PathBuf::from("/tmp"));
        }
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);

        // The user pans the canvas away from the workspace.
        app.canvas_view.set_pan_offset([0.0, 0.0]);

        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let new_panel = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("new panel");

        // Like a sidebar selection: the view moves to the new panel and the
        // panel is focused.
        let pan_moved = app.canvas_view.pan_offset[0].abs() + app.canvas_view.pan_offset[1].abs();
        assert!(pan_moved > 1.0, "view should have panned to the new panel");
        assert_eq!(app.board.focused, Some(new_panel));

        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));
        let canvas_rect = app.canvas_rect(&ctx);
        let panel = app.board.panel(new_panel).expect("panel");
        let screen_min = app.canvas_to_screen(
            canvas_rect,
            egui::Pos2::new(panel.layout.position[0], panel.layout.position[1]),
        );
        let screen_rect = egui::Rect::from_min_size(
            screen_min,
            app.canvas_size_to_screen(egui::Vec2::new(panel.layout.size[0], panel.layout.size[1])),
        );
        assert!(
            canvas_rect.intersects(screen_rect),
            "new panel should be visible after creation: panel screen {screen_rect:?} vs canvas {canvas_rect:?}"
        );
    }

    #[test]
    fn titlebar_click_reveals_panel_the_same_way_as_sidebar() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let viewport = [1600.0, 1000.0];
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "click-reveal");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;

        app.canvas_view
            .set_pan_offset([reveal_pan[0] + 120.0, reveal_pan[1] + 80.0]);
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
        let titlebar = titlebar_click_pos(&app, panel_id);
        click_screen_pos(&ctx, &mut app, viewport, titlebar);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            reveal_pan,
            "titlebar click should restore the sidebar reveal pan",
        );
    }

    #[test]
    fn panel_body_click_focuses_without_revealing() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let viewport = [1600.0, 1000.0];
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "body-click");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;

        let nudged = [reveal_pan[0] + 120.0, reveal_pan[1] + 80.0];
        app.canvas_view.set_pan_offset(nudged);
        run_app_frame_with_input(&ctx, &mut app, raw_input(viewport, Some([0.0, 0.0])));
        let body = body_click_pos(&app, panel_id);
        click_screen_pos(&ctx, &mut app, viewport, body);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            nudged,
            "body click should keep the nudged pan instead of restoring the sidebar reveal pose",
        );
        assert!(
            !pan_near(app.canvas_view.pan_offset, reveal_pan),
            "body click must not snap back to the reveal pan {reveal_pan:?}"
        );
    }

    #[test]
    fn reveal_selected_panel_does_not_pan_the_root_canvas_for_detached_workspaces() {
        use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], Some([0.0, 0.0])));

        let workspace_id = seeded_workspace(&mut app, "detached-reveal");
        app.add_panel_to_workspace(&ctx, workspace_id, shell_preset(), None);
        let panel_id = app
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.panels.last().copied())
            .expect("panel");
        let reveal_pan = app.canvas_view.pan_offset;
        app.detach_workspace(workspace_id);

        let nudged = [reveal_pan[0] + 120.0, reveal_pan[1] + 80.0];
        app.canvas_view.set_pan_offset(nudged);
        app.reveal_selected_panel(&ctx, panel_id);

        assert_eq!(app.board.focused, Some(panel_id));
        assert_pan_near(
            app.canvas_view.pan_offset,
            nudged,
            "detached reveal should focus the panel without panning the root canvas",
        );
    }

    fn seeded_workspace(app: &mut crate::app::HorizonApp, name: &str) -> horizon_core::WorkspaceId {
        let workspace_id = app.board.create_workspace(name);
        {
            let workspace = app.board.workspace_mut(workspace_id).expect("workspace");
            workspace.cwd = Some(std::path::PathBuf::from("/tmp"));
        }
        workspace_id
    }

    fn titlebar_click_pos(app: &crate::app::HorizonApp, panel_id: horizon_core::PanelId) -> egui::Pos2 {
        let rect = app
            .panel_screen_rects
            .get(&panel_id)
            .copied()
            .expect("panel screen rect");
        egui::Pos2::new(
            rect.min.x + 80.0,
            rect.min.y + super::super::super::PANEL_TITLEBAR_HEIGHT * 0.5,
        )
    }

    fn body_click_pos(app: &crate::app::HorizonApp, panel_id: horizon_core::PanelId) -> egui::Pos2 {
        let rect = app
            .panel_screen_rects
            .get(&panel_id)
            .copied()
            .expect("panel screen rect");
        egui::Pos2::new(
            rect.min.x + 80.0,
            rect.min.y + super::super::super::PANEL_TITLEBAR_HEIGHT + 40.0,
        )
    }

    fn pan_near(actual: [f32; 2], expected: [f32; 2]) -> bool {
        (actual[0] - expected[0]).abs() < 1.0 && (actual[1] - expected[1]).abs() < 1.0
    }

    fn assert_pan_near(actual: [f32; 2], expected: [f32; 2], message: &str) {
        assert!(pan_near(actual, expected), "{message}: {expected:?} vs {actual:?}");
    }

    fn click_screen_pos(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2], pos: egui::Pos2) {
        use crate::app::test_support::run_app_frame_with_input;

        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, None));
        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, Some(true)));
        run_app_frame_with_input(ctx, app, pointer_input(viewport, pos, Some(false)));
    }

    fn pointer_input(viewport: [f32; 2], pos: egui::Pos2, pressed: Option<bool>) -> egui::RawInput {
        use crate::app::test_support::raw_input;

        let mut input = raw_input(viewport, Some([0.0, 0.0]));
        input.events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        input
    }
}
