use egui::{Context, Pos2, Rect, Vec2, ViewportId};
use horizon_core::{CardPlace, PanelId, PanelKind, WorkspaceId};

use crate::app::speech::SpeechWindow;
use crate::app::{HorizonApp, RenameEditAction, util::clamp_panel_size};
use crate::terminal_widget::viewport_for_available_space;

use super::{PanelCommand, PanelFocusRequest, PanelFrame, PanelSnapshot, PanelUiOutcome, open_session_picker};

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
        // A card that is no tile of the preset moves alone, as in a free workspace.
        let tile = self.board.panel(panel_id).is_some_and(|panel| {
            panel.workspace_id == workspace_id && panel.block().is_none_or(|card| card.place == CardPlace::Tile)
        });
        tile && self
            .board
            .workspace(workspace_id)
            .is_some_and(|workspace| workspace.layout.is_some())
    }

    fn reorder_arranged_panel(&mut self, panel_id: PanelId, position: [f32; 2]) -> bool {
        self.board.reorder_arranged_slot(panel_id, position)
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
            // The panel of the icon takes the focus, as a press on its title bar does.
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
            outcome.resize_delta = super::resize::drag_delta(resize_response);
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

    /// The browser items of the panel menu: another page of the profile of this
    /// panel, and a private panel that keeps nothing.
    fn show_browser_items(&mut self, ui: &mut egui::Ui, panel_id: PanelId, outcome: &mut PanelUiOutcome) {
        let browser = self.board.panel(panel_id).and_then(|panel| panel.browser());
        let supported = browser.is_some_and(horizon_core::browser::BrowserPanelState::can_duplicate);
        let private = browser.is_some_and(horizon_core::browser::BrowserPanelState::can_open_private);
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
        if ui
            .add_enabled(private, egui::Button::new("Private panel"))
            .on_hover_text(
                "Open the same URL in a panel of its own: it is signed in nowhere, it reads none of the shared \
                 sign-ins, and its profile is gone when the panel closes.",
            )
            .clicked()
        {
            outcome.private_browser = true;
            ui.close();
        }
        ui.separator();
    }

    pub(super) fn show_panel_context_menu(
        &mut self,
        drag_response: &egui::Response,
        panel_id: PanelId,
        current_workspace_id: WorkspaceId,
        kind: PanelKind,
        outcome: &mut PanelUiOutcome,
    ) {
        let popup =
            egui::Popup::context_menu(drag_response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        popup.show(|ui| {
            ui.set_min_width(180.0);
            if kind == PanelKind::Browser {
                self.show_browser_items(ui, panel_id, outcome);
            }
            if let Some(workspace) = self.show_workspace_destination(ui, drag_response, current_workspace_id) {
                outcome.workspace_assignment = Some(workspace);
            }

            ui.separator();
            // Compute rebind options lazily — only when the context menu is
            // actually open instead of every frame for every panel.
            let rebind_options = self.session_rebind_options(panel_id);
            if kind.supports_session_binding() {
                let response = ui.button(format!("Resume a session ({})…", rebind_options.len()));
                if response.clicked() {
                    open_session_picker(&response, panel_id, rebind_options);
                    ui.close();
                }
                ui.separator();
            }
            if ui.button("New Workspace").clicked() {
                outcome.command = Some(PanelCommand::CreateWorkspace);
                ui.close();
            }
            self.render_work_resume_controls(ui, panel_id);
            if kind.is_agent() {
                ui.separator();
                let idle = self
                    .board
                    .panel(panel_id)
                    .is_some_and(|panel| self.panel_attention(&panel.local_id) == horizon_core::Attention::Idle);
                if ui
                    .add_enabled(idle, egui::Button::new("Mark as unread"))
                    .on_disabled_hover_text("This agent works, waits for you or is already unread.")
                    .on_hover_text("Shows this panel as done in the sidebar until you look at it again.")
                    .clicked()
                {
                    self.mark_panel_unread(panel_id);
                    ui.close();
                }
            }
            if kind.is_agent() || kind == PanelKind::Ssh {
                ui.separator();
                let restart_label = if kind == PanelKind::Ssh { "Reconnect" } else { "Restart" };
                if ui.button(restart_label).clicked() {
                    self.reconnect_panel_now(panel_id);
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
        window: SpeechWindow,
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
            // A card keeps the size its person chose, whatever its content needs.
            if self
                .board
                .panel(panel_id)
                .is_some_and(|panel| panel.kind == PanelKind::Block)
            {
                self.panel_render_caches
                    .card_runtime
                    .entry(panel_id)
                    .or_default()
                    .mark_hand_resized();
            }
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
        if outcome.mic_clicked {
            // The press is applied after the focus of the panel, so a take that the
            // icon starts belongs to this panel as a take of the hotkey does.
            self.click_speech_mic(ctx, panel_id, window);
        }
        if matches!(outcome.command, Some(PanelCommand::CreateWorkspace)) {
            self.workspace_creates.push(panel_id);
        }
        if outcome.duplicate_browser || outcome.private_browser {
            self.open_panel_beside(ctx, panel_id, snapshot.current_workspace_id, outcome.private_browser);
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

    /// Open a browser panel beside the source with its page: another page of the
    /// profile of that panel, or a private panel that is signed in nowhere.
    fn open_panel_beside(&mut self, ctx: &Context, panel_id: PanelId, workspace_id: WorkspaceId, private: bool) {
        let options = if private {
            self.board
                .panel(panel_id)
                .and_then(|panel| panel.browser())
                .and_then(horizon_core::browser::BrowserPanelState::private_panel_options)
        } else {
            None
        };
        let opened = match (private, options) {
            // A private panel of a browser host is a page of that host, so it
            // takes the login of the machine of the list as every browser panel
            // of the person does.
            (true, Some(mut options)) => self
                .resolve_browser_host_login(&mut options)
                .map_err(horizon_core::Error::Config)
                .and_then(|()| self.board.create_panel(options, workspace_id)),
            (true, None) => return,
            (false, _) => self.board.duplicate_browser_panel(panel_id),
        };
        match opened {
            Ok(opened) => {
                self.reveal_selected_panel(ctx, opened);
                self.mark_runtime_dirty();
                ctx.request_repaint();
            }
            Err(error) => tracing::warn!(%error, "could not open a panel beside the browser panel"),
        }
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
}
