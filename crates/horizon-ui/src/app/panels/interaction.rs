use egui::{Context, Pos2, Rect, Vec2, ViewportId};
use horizon_core::{PanelId, PanelKind, WorkspaceId};

use crate::app::{HorizonApp, RenameEditAction, util::clamp_panel_size};
use crate::terminal_widget::viewport_for_available_space;
use crate::theme;

use super::{
    PanelCommand, PanelFocusRequest, PanelFrame, PanelSnapshot, PanelUiOutcome, render_session_rebind_options,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct ArrangedPanelDrag {
    panel_id: PanelId,
    workspace_id: WorkspaceId,
    viewport_id: ViewportId,
    preview_position: Pos2,
}

impl ArrangedPanelDrag {
    fn new(panel_id: PanelId, workspace_id: WorkspaceId, viewport_id: ViewportId, preview_position: Pos2) -> Self {
        Self {
            panel_id,
            workspace_id,
            viewport_id,
            preview_position,
        }
    }

    fn matches(self, panel_id: PanelId, workspace_id: WorkspaceId, viewport_id: ViewportId) -> bool {
        self.panel_id == panel_id && self.workspace_id == workspace_id && self.viewport_id == viewport_id
    }

    fn preview_for(self, panel_id: PanelId, workspace_id: WorkspaceId) -> Option<Pos2> {
        (self.panel_id == panel_id && self.workspace_id == workspace_id).then_some(self.preview_position)
    }

    fn belongs_to(self, panel_id: PanelId, workspace_id: WorkspaceId) -> bool {
        self.panel_id == panel_id && self.workspace_id == workspace_id
    }

    fn advance(&mut self, delta: Vec2) -> Pos2 {
        self.preview_position += delta;
        self.preview_position
    }
}

impl HorizonApp {
    pub(super) fn clear_inactive_arranged_panel_drag(&mut self, ctx: &Context, panel_id: PanelId) {
        let should_clear = self.arranged_panel_drag.is_some_and(|drag| {
            drag.panel_id == panel_id
                && (drag.viewport_id != ctx.viewport_id() || !ctx.input(|input| input.pointer.primary_down()))
        });
        if should_clear {
            self.arranged_panel_drag = None;
            ctx.request_repaint();
        }
    }

    pub(in crate::app) fn arranged_panel_position(
        &self,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        fallback: Pos2,
    ) -> Pos2 {
        let Some(preview_position) = self
            .arranged_panel_drag
            .and_then(|drag| drag.preview_for(panel_id, workspace_id))
        else {
            return fallback;
        };

        if self.panel_is_arranged(panel_id, workspace_id) {
            preview_position
        } else {
            fallback
        }
    }

    fn panel_is_arranged(&self, panel_id: PanelId, workspace_id: WorkspaceId) -> bool {
        if !self
            .board
            .panel(panel_id)
            .is_some_and(|panel| panel.workspace_id == workspace_id)
        {
            return false;
        }
        #[cfg(feature = "cloud-workspaces")]
        if let Some(group) = self.cloud_prototype.groups.group_for_panel(&self.board, panel_id) {
            return group.layout.is_some() && !group.collapsed;
        }
        self.board
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.layout.is_some())
    }

    fn reorder_arranged_panel(&mut self, panel_id: PanelId, position: [f32; 2]) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        if self.cloud_prototype.groups.contains_panel(&self.board, panel_id) {
            let changed = self
                .cloud_prototype
                .groups
                .reorder_panel_at(&mut self.board, panel_id, position);
            if changed {
                self.save_cloud_prototype();
            }
            return changed;
        }
        self.board
            .arranged_panel_collision_target(panel_id, position)
            .is_some_and(|target| self.board.swap_arranged_panels(panel_id, target))
    }

    pub(super) fn update_panel_interactions(
        is_renaming: bool,
        drag_response: &egui::Response,
        close_response: &egui::Response,
        mic_response: Option<&egui::Response>,
        resize_response: &egui::Response,
        outcome: &mut PanelUiOutcome,
    ) {
        if mic_response.is_some_and(egui::Response::clicked) {
            outcome.mic_clicked = true;
            outcome.request_focus();
        }
        if resize_response.drag_started() || resize_response.clicked() {
            outcome.request_focus();
        }
        if !is_renaming && drag_response.clicked() {
            outcome.request_focus();
        }
        if !is_renaming && drag_response.drag_started() {
            outcome.request_focus();
            outcome.drag.started = true;
        }
        if !is_renaming && drag_response.dragged() {
            outcome.drag.delta = drag_response.drag_delta();
        }
        if !is_renaming && drag_response.drag_stopped() {
            outcome.drag.stopped = true;
        }
        if resize_response.dragged() {
            outcome.resize_delta = resize_response.drag_delta();
        }
        if resize_response.drag_stopped() {
            outcome.commit_terminal_resize = true;
        }
        if close_response.clicked() {
            outcome.command = Some(PanelCommand::Close);
        }
        if !is_renaming && drag_response.double_clicked() {
            outcome.command = Some(PanelCommand::StartRename);
            outcome.request_focus();
        }
    }

    pub(super) fn show_panel_context_menu(
        &mut self,
        drag_response: &egui::Response,
        panel_id: PanelId,
        current_workspace_id: WorkspaceId,
        kind: PanelKind,
        outcome: &mut PanelUiOutcome,
    ) {
        drag_response.context_menu(|ui| {
            ui.set_min_width(180.0);
            if kind == PanelKind::Browser {
                let supported = self
                    .board
                    .panel(panel_id)
                    .and_then(|panel| panel.browser())
                    .is_some_and(horizon_core::browser::BrowserPanelState::can_duplicate);
                if ui
                    .add_enabled(supported, egui::Button::new("Duplicate panel"))
                    .on_hover_text(
                        "Open the same URL in a new panel, sharing login and website storage. Requires local Chromium or Firefox.",
                    )
                    .clicked()
                {
                    outcome.duplicate_browser = true;
                    ui.close();
                }
                ui.separator();
            }
            ui.label(
                egui::RichText::new("Move to Workspace")
                    .size(11.0)
                    .color(theme::FG_DIM()),
            );
            ui.separator();

            // Look up workspace names lazily — this closure only runs when the
            // context menu is actually open, so the per-workspace iteration and
            // formatting cost is not paid on every frame.
            for workspace in &self.board.workspaces {
                let workspace_color = theme::workspace_accent(workspace.color_idx);
                let is_current = current_workspace_id == workspace.id;
                // The name stays in the theme's text color: some workspace
                // accents do not hold 4.5:1 for 12 px text on the panel background.
                let font_id = egui::FontId::new(12.0, egui::FontFamily::Proportional);
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    if is_current { "\u{25cf} " } else { "  " },
                    0.0,
                    egui::text::TextFormat {
                        font_id: font_id.clone(),
                        color: workspace_color,
                        ..Default::default()
                    },
                );
                job.append(
                    &workspace.name,
                    0.0,
                    egui::text::TextFormat {
                        font_id,
                        color: if is_current { theme::FG() } else { theme::FG_SOFT() },
                        ..Default::default()
                    },
                );
                let text = egui::WidgetText::LayoutJob(std::sync::Arc::new(job));
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    outcome.workspace_assignment = Some(workspace.id);
                    ui.close();
                }
            }

            ui.separator();
            // Compute rebind options lazily — only when the context menu is
            // actually open instead of every frame for every panel.
            let rebind_options = self.session_rebind_options(panel_id);
            if !rebind_options.is_empty() {
                outcome.session_rebind_and_restart = render_session_rebind_options(ui, &rebind_options).binding;
                ui.separator();
            }
            if ui.button("New Workspace").clicked() {
                outcome.command = Some(PanelCommand::CreateWorkspace);
                ui.close();
            }
            self.render_work_resume_controls(ui, panel_id);
            if kind.is_agent() || kind == PanelKind::Ssh {
                ui.separator();
                let restart_label = if kind == PanelKind::Ssh { "Reconnect" } else { "Restart" };
                if ui.button(restart_label).clicked() {
                    self.queue_panel_restart(panel_id);
                    ui.close();
                }
            }
            if kind == PanelKind::Browser {
                ui.separator();
                let safari = self
                    .board
                    .panel(panel_id)
                    .and_then(horizon_core::Panel::browser)
                    .is_some_and(|browser| browser.backend() == horizon_core::browser::BackendKind::SafariWebDriver);
                if safari {
                    ui.add_enabled(false, egui::Button::new("Teach routine"))
                        .on_hover_text("Safari cannot record Teach routines");
                } else if ui.button("Teach routine").clicked() {
                    if let Some(panel) = self.board.panel_mut(panel_id)
                        && let Some(browser) = panel.browser_mut()
                        && let Err(error) = browser.start_teach("untitled")
                    {
                        tracing::warn!(target: "browser", "teach start failed: {error}");
                    }
                    ui.close();
                }
            }
        });
    }

    fn resize_panel_in_environment(
        &mut self,
        panel_id: PanelId,
        size: [f32; 2],
        workspace_collision_ids: &[WorkspaceId],
    ) {
        #[cfg(feature = "cloud-workspaces")]
        if !self
            .cloud_prototype
            .groups
            .resize_panel(&mut self.board, panel_id, size)
        {
            self.resize_ordinary_panel(panel_id, size, workspace_collision_ids);
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        let _ = self
            .board
            .resize_panel_with_workspace_scope(panel_id, size, workspace_collision_ids);
    }

    pub(super) fn apply_panel_outcome(
        &mut self,
        ctx: &Context,
        panel_id: PanelId,
        snapshot: &PanelSnapshot,
        outcome: &PanelUiOutcome,
        workspace_collision_ids: &[WorkspaceId],
    ) -> bool {
        self.panel_screen_rects.insert(panel_id, snapshot.screen_rect);
        if let Some(body_rect) = snapshot.terminal_body_screen_rect {
            self.terminal_body_screen_rects.insert(panel_id, body_rect);
        }
        self.panel_screen_order.push(panel_id);

        if matches!(outcome.command, Some(PanelCommand::StartRename)) {
            self.clear_workspace_rename();
            self.renaming_panel = Some(panel_id);
            if let Some(panel) = self.board.panel(panel_id) {
                self.panel_rename_buffer.clone_from(&panel.title);
            }
        }

        match outcome.rename_action {
            RenameEditAction::Commit => {
                if self.renaming_panel == Some(panel_id) {
                    let name = self.panel_rename_buffer.trim().to_string();
                    if !name.is_empty() && self.board.rename_panel(panel_id, &name) {
                        self.mark_runtime_dirty();
                    }
                    self.clear_panel_rename();
                }
            }
            RenameEditAction::Cancel => {
                if self.renaming_panel == Some(panel_id) {
                    self.clear_panel_rename();
                }
            }
            RenameEditAction::None => {}
        }

        self.apply_panel_drag(
            ctx,
            panel_id,
            snapshot.current_workspace_id,
            snapshot.canvas_position,
            outcome,
        );
        if !self.canvas_pan_input_claimed && outcome.resize_delta != Vec2::ZERO {
            let new_size = clamp_panel_size(snapshot.canvas_size + outcome.resize_delta);
            self.resize_panel_in_environment(panel_id, [new_size.x, new_size.y], workspace_collision_ids);
            self.mark_runtime_dirty();
        }
        if outcome.commit_terminal_resize {
            let resized_panel_size = if outcome.resize_delta == Vec2::ZERO {
                snapshot.canvas_size
            } else {
                clamp_panel_size(snapshot.canvas_size + outcome.resize_delta)
            };
            let panel_rect = Rect::from_min_size(Pos2::ZERO, resized_panel_size);
            let body_size = PanelFrame::new(panel_rect).body.size();
            let viewport = viewport_for_available_space(ctx, body_size);
            if let Some(panel) = self.board.panel_mut(panel_id) {
                panel.resize_immediately(viewport.rows, viewport.cols, viewport.cell_width, viewport.cell_height);
            }
            ctx.request_repaint();
        }
        match outcome.focus {
            PanelFocusRequest::Focus => {
                self.board.focus(panel_id);
            }
            PanelFocusRequest::None => {}
        }
        if outcome.mic_clicked
            && let Some(speech) = self.speech.as_mut()
        {
            speech.toggle(panel_id);
            // The hotkey handler already made its repaint decision this
            // frame; keep frames coming so the new recording's pulse and
            // polling start immediately.
            ctx.request_repaint();
        }
        if matches!(outcome.command, Some(PanelCommand::CreateWorkspace)) {
            self.workspace_creates.push(panel_id);
        }
        if outcome.duplicate_browser {
            match self.board.duplicate_browser_panel(panel_id) {
                Ok(duplicate) => {
                    self.reveal_selected_panel(ctx, duplicate);
                    self.mark_runtime_dirty();
                    ctx.request_repaint();
                }
                Err(error) => tracing::warn!(%error, "could not duplicate browser panel"),
            }
        }
        if let Some(workspace_id) = outcome.workspace_assignment {
            self.workspace_assignments.push((panel_id, workspace_id));
        }
        if let Some(binding) = outcome.session_rebind_and_restart.clone()
            && self.rebind_and_restart_panel_session(panel_id, binding)
        {
            self.mark_runtime_dirty();
            ctx.request_repaint();
        }

        matches!(outcome.command, Some(PanelCommand::Close))
    }

    fn apply_panel_drag(
        &mut self,
        ctx: &Context,
        panel_id: PanelId,
        workspace_id: WorkspaceId,
        canvas_position: Pos2,
        outcome: &PanelUiOutcome,
    ) {
        let viewport_id = ctx.viewport_id();
        let arranged = self.panel_is_arranged(panel_id, workspace_id);
        let active_drag_matches = self
            .arranged_panel_drag
            .is_some_and(|drag| drag.matches(panel_id, workspace_id, viewport_id));
        let drag_belongs_to_panel = self
            .arranged_panel_drag
            .is_some_and(|drag| drag.belongs_to(panel_id, workspace_id));

        if drag_belongs_to_panel && !active_drag_matches {
            self.arranged_panel_drag = None;
            ctx.request_repaint();
        }

        if self.canvas_pan_input_claimed {
            if drag_belongs_to_panel {
                self.arranged_panel_drag = None;
                ctx.request_repaint();
            }
            return;
        }

        if !arranged {
            if drag_belongs_to_panel {
                self.arranged_panel_drag = None;
                ctx.request_repaint();
            }
            if outcome.drag.delta != Vec2::ZERO {
                let new_position = canvas_position + outcome.drag.delta;
                let _ = self.board.move_panel(panel_id, [new_position.x, new_position.y]);
                self.mark_runtime_dirty();
            }
            return;
        }

        if outcome.drag.started || (outcome.drag.delta != Vec2::ZERO && !active_drag_matches) {
            let canonical_position = self.board.panel(panel_id).map_or(canvas_position, |panel| {
                Pos2::new(panel.layout.position[0], panel.layout.position[1])
            });
            self.arranged_panel_drag = Some(ArrangedPanelDrag::new(
                panel_id,
                workspace_id,
                viewport_id,
                canonical_position,
            ));
        }

        if outcome.drag.delta != Vec2::ZERO {
            // Egui reports the transformed Area's drag delta in canvas space,
            // so applying it directly stays correct at every zoom level.
            let preview_position = self
                .arranged_panel_drag
                .as_mut()
                .filter(|drag| drag.matches(panel_id, workspace_id, viewport_id))
                .map(|drag| drag.advance(outcome.drag.delta));
            if let Some(preview_position) = preview_position {
                let position = [preview_position.x, preview_position.y];
                if self.reorder_arranged_panel(panel_id, position) {
                    self.mark_runtime_dirty();
                }
                ctx.request_repaint();
            }
        }

        if outcome.drag.stopped
            && self
                .arranged_panel_drag
                .is_some_and(|drag| drag.matches(panel_id, workspace_id, viewport_id))
        {
            self.arranged_panel_drag = None;
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{Board, PanelKind, PanelOptions, WorkspaceLayout};

    use super::super::PanelDragOutcome;
    use crate::app::test_support::test_app;

    use super::*;

    fn editor_panel_options() -> PanelOptions {
        PanelOptions {
            kind: PanelKind::Editor,
            ..PanelOptions::default()
        }
    }

    #[test]
    fn arranged_drag_tracks_only_its_panel_workspace_and_viewport() {
        let panel_id = PanelId(7);
        let workspace_id = WorkspaceId(11);
        let mut drag = ArrangedPanelDrag::new(panel_id, workspace_id, ViewportId::ROOT, Pos2::new(120.0, 80.0));

        assert_eq!(drag.preview_for(panel_id, workspace_id), Some(Pos2::new(120.0, 80.0)));
        assert_eq!(drag.preview_for(PanelId(8), workspace_id), None);
        assert_eq!(drag.preview_for(panel_id, WorkspaceId(12)), None);
        assert!(drag.matches(panel_id, workspace_id, ViewportId::ROOT));
        assert!(!drag.matches(panel_id, workspace_id, ViewportId::from_hash_of("detached")));

        assert_eq!(drag.advance(Vec2::new(24.0, -16.0)), Pos2::new(144.0, 64.0));
    }

    #[test]
    fn stale_viewport_or_pointer_release_clears_only_the_source_panel_drag() {
        let (_temp, mut app) = test_app();
        let panel_id = PanelId(7);
        let workspace_id = WorkspaceId(11);
        let root_ctx = Context::default();
        app.arranged_panel_drag = Some(ArrangedPanelDrag::new(
            panel_id,
            workspace_id,
            ViewportId::from_hash_of("detached"),
            Pos2::ZERO,
        ));

        app.clear_inactive_arranged_panel_drag(&root_ctx, panel_id);
        assert!(app.arranged_panel_drag.is_none());

        app.arranged_panel_drag = Some(ArrangedPanelDrag::new(
            panel_id,
            workspace_id,
            ViewportId::ROOT,
            Pos2::ZERO,
        ));
        app.clear_inactive_arranged_panel_drag(&root_ctx, PanelId(8));
        assert!(app.arranged_panel_drag.is_some());

        app.clear_inactive_arranged_panel_drag(&root_ctx, panel_id);
        assert!(app.arranged_panel_drag.is_none());
    }

    #[test]
    fn arranged_drag_swaps_slots_without_clearing_the_layout() {
        let (_temp, mut app) = test_app();
        app.board = Board::new();
        let workspace_id = app.board.create_workspace("grid");
        let source = app
            .board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("source panel should spawn");
        let target = app
            .board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("target panel should spawn");
        app.board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);
        let source_position = app.board.panel(source).expect("source panel").layout.position;
        let target_position = app.board.panel(target).expect("target panel").layout.position;
        let outcome = PanelUiOutcome {
            drag: PanelDragOutcome {
                started: true,
                delta: Vec2::new(
                    target_position[0] - source_position[0],
                    target_position[1] - source_position[1],
                ),
                ..PanelDragOutcome::default()
            },
            ..PanelUiOutcome::default()
        };

        app.apply_panel_drag(
            &Context::default(),
            source,
            workspace_id,
            Pos2::new(source_position[0], source_position[1]),
            &outcome,
        );

        let workspace = app.board.workspace(workspace_id).expect("workspace");
        assert_eq!(workspace.layout, Some(WorkspaceLayout::Grid));
        assert_eq!(workspace.panels, [target, source]);
        assert!(app.arranged_panel_drag.is_some());

        app.apply_panel_drag(
            &Context::default(),
            source,
            workspace_id,
            Pos2::new(target_position[0], target_position[1]),
            &PanelUiOutcome {
                drag: PanelDragOutcome {
                    stopped: true,
                    ..PanelDragOutcome::default()
                },
                ..PanelUiOutcome::default()
            },
        );
        assert!(app.arranged_panel_drag.is_none());
    }

    #[cfg(feature = "cloud-workspaces")]
    #[test]
    fn cloud_arranged_drag_previews_reorders_and_persists_its_own_slots() {
        use horizon_core::cloud_panel::{CloudGroup, CloudGroups};
        for layout in [WorkspaceLayout::Rows, WorkspaceLayout::Columns, WorkspaceLayout::Grid] {
            let (_temp, mut app) = test_app();
            let workspace = app.board.create_workspace("Cloud fixture");
            let source = app.board.create_panel(editor_panel_options(), workspace).unwrap();
            let target = app.board.create_panel(editor_panel_options(), workspace).unwrap();
            let locals = [source, target].map(|id| app.board.panel(id).unwrap().local_id.clone());
            let mut group = CloudGroup::new(
                1,
                "Fixture".into(),
                app.board.workspace(workspace).unwrap().local_id.clone(),
                "/fixture".into(),
                [0.0, 0.0],
            );
            group.panels = locals.to_vec();
            group.set_layout(&mut app.board, Some(layout));
            app.board.cloud_groups = CloudGroups(vec![group]);
            let ctx = Context::default();
            app.prepare_cloud_prototype(&ctx);
            assert_eq!(app.board.workspace(workspace).unwrap().layout, None);
            let from = app.board.panel(source).unwrap().layout.position;
            let to = app.board.panel(target).unwrap().layout.position;
            app.apply_panel_drag(
                &ctx,
                source,
                workspace,
                Pos2::from(from),
                &PanelUiOutcome {
                    drag: PanelDragOutcome {
                        started: true,
                        delta: Vec2::new(to[0] - from[0], to[1] - from[1]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            assert_eq!(
                app.arranged_panel_position(source, workspace, Pos2::ZERO),
                Pos2::from(to)
            );
            assert_eq!(
                app.board.cloud_groups.0[0].panels,
                [locals[1].clone(), locals[0].clone()]
            );
            app.prepare_cloud_prototype(&ctx);
            assert_eq!(
                Pos2::from(app.board.panel(source).unwrap().layout.position),
                Pos2::from(to)
            );
            assert_eq!(
                Pos2::from(app.board.panel(target).unwrap().layout.position),
                Pos2::from(from)
            );
            assert_eq!(app.cloud_prototype.groups.0[0].layout, Some(layout));
            app.apply_panel_drag(
                &ctx,
                source,
                workspace,
                Pos2::from(to),
                &PanelUiOutcome {
                    drag: PanelDragOutcome {
                        stopped: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            assert!(app.arranged_panel_drag.is_none());
            app.cloud_prototype.groups.0[0].set_layout(&mut app.board, None);
            app.apply_panel_drag(
                &ctx,
                source,
                workspace,
                Pos2::from(to),
                &PanelUiOutcome {
                    drag: PanelDragOutcome {
                        started: true,
                        delta: Vec2::new(30.0, 20.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            assert!(app.arranged_panel_drag.is_none());
            assert_eq!(
                Pos2::from(app.board.panel(source).unwrap().layout.position),
                Pos2::new(to[0] + 30.0, to[1] + 20.0)
            );
        }
    }

    #[test]
    fn default_layout_drag_keeps_freeform_movement() {
        let (_temp, mut app) = test_app();
        app.board = Board::new();
        let workspace_id = app.board.create_workspace("freeform");
        let panel_id = app
            .board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("panel should spawn");
        assert!(app.board.clear_workspace_layout(workspace_id));
        let original = app.board.panel(panel_id).expect("panel").layout.position;

        app.apply_panel_drag(
            &Context::default(),
            panel_id,
            workspace_id,
            Pos2::new(original[0], original[1]),
            &PanelUiOutcome {
                drag: PanelDragOutcome {
                    started: true,
                    delta: Vec2::new(30.0, 20.0),
                    ..PanelDragOutcome::default()
                },
                ..PanelUiOutcome::default()
            },
        );

        assert_eq!(app.board.workspace(workspace_id).expect("workspace").layout, None);
        let moved = app.board.panel(panel_id).expect("panel").layout.position;
        assert!(
            (moved[0] - original[0] - 30.0).abs() <= f32::EPSILON
                && (moved[1] - original[1] - 20.0).abs() <= f32::EPSILON,
            "expected the panel to move by [30, 20], got {moved:?} from {original:?}"
        );
        assert!(app.arranged_panel_drag.is_none());
    }

    #[cfg(feature = "cloud-workspaces")]
    #[test]
    fn ordinary_panel_resize_pushes_the_live_cloud_frame() {
        use horizon_core::cloud_panel::{CloudGroup, CloudGroups};
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace_at("Cloud fixture", [0.0, 0.0]);
        let panel = app
            .board
            .create_panel(
                PanelOptions {
                    position: Some([60.0, 120.0]),
                    size: Some([480.0, 360.0]),
                    ..editor_panel_options()
                },
                workspace,
            )
            .unwrap();
        let local_id = app.board.workspace(workspace).unwrap().local_id.clone();
        let group = CloudGroup::new(101, "Fixture".into(), local_id, "/fixture".into(), [610.0, 120.0]);
        app.board.cloud_groups = CloudGroups(vec![group]);
        let ctx = Context::default();
        app.prepare_cloud_prototype(&ctx);
        // Only the live copy moves, so resolving against the board's stale copy would push the cloud.
        app.cloud_prototype.groups.0[0].translate(&mut app.board, [400.0, 0.0]);

        app.resize_panel_in_environment(panel, [760.0, 360.0], &[workspace]);
        assert_eq!(
            Pos2::from(app.cloud_prototype.groups.0[0].position),
            Pos2::new(1010.0, 120.0)
        );

        app.resize_panel_in_environment(panel, [1000.0, 360.0], &[workspace]);
        app.prepare_cloud_prototype(&ctx);

        let cloud = &app.cloud_prototype.groups.0[0];
        assert!(
            cloud.overview_bounds().0[0] >= 60.0 + 1000.0,
            "cloud frame still overlaps the grown panel at {:?}",
            cloud.position
        );
        assert_eq!(
            Pos2::from(app.board.cloud_groups.0[0].position),
            Pos2::from(cloud.position)
        );
    }
}
