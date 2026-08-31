use egui::{Context, Pos2, Rect, Vec2};
use horizon_core::{PanelId, PanelKind, WorkspaceId};

use crate::app::{HorizonApp, RenameEditAction, util::clamp_panel_size};
use crate::terminal_widget::viewport_for_available_space;
use crate::theme;

use super::{PanelCommand, PanelFrame, PanelSnapshot, PanelUiOutcome, render_session_rebind_options};

impl HorizonApp {
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
            outcome.focus_requested = true;
        }
        if resize_response.drag_started() || resize_response.clicked() {
            outcome.focus_requested = true;
        }
        if !is_renaming && (drag_response.clicked() || drag_response.drag_started()) {
            outcome.focus_requested = true;
        }
        if !is_renaming && drag_response.dragged() {
            outcome.drag_delta = drag_response.drag_delta();
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
            outcome.focus_requested = true;
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
            if kind.is_agent() || kind == PanelKind::Ssh {
                ui.separator();
                let restart_label = if kind == PanelKind::Ssh { "Reconnect" } else { "Restart" };
                if ui.button(restart_label).clicked() {
                    self.queue_panel_restart(panel_id);
                    ui.close();
                }
            }
        });
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

        if !self.canvas_pan_input_claimed && outcome.drag_delta != Vec2::ZERO {
            let new_position = snapshot.canvas_position + outcome.drag_delta;
            let _ = self.board.move_panel(panel_id, [new_position.x, new_position.y]);
            self.mark_runtime_dirty();
        }
        if !self.canvas_pan_input_claimed && outcome.resize_delta != Vec2::ZERO {
            let new_size = clamp_panel_size(snapshot.canvas_size + outcome.resize_delta);
            let _ = self.board.resize_panel_with_workspace_scope(
                panel_id,
                [new_size.x, new_size.y],
                workspace_collision_ids,
            );
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
        if outcome.focus_requested {
            self.board.focus(panel_id);
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
}
