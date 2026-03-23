use std::path::PathBuf;

use egui::{Context, Id, Margin, Order, Pos2, Rect, Stroke};
use horizon_core::WorkspaceId;

use crate::app::HorizonApp;
use crate::dir_picker::{DirPicker, DirPickerAction, DirPickerPurpose};
use crate::theme;

use super::PresetPickerAction;
use super::support::{preset_picker_heading, render_grouped_preset_rows};

impl HorizonApp {
    pub(in crate::app) fn render_dir_picker(&mut self, ctx: &Context) {
        let Some(picker) = self.dir_picker.as_mut() else {
            return;
        };

        match picker.show(ctx) {
            DirPickerAction::None => {}
            DirPickerAction::Cancelled => self.dir_picker = None,
            DirPickerAction::Selected(path, purpose) => {
                self.dir_picker = None;
                self.execute_dir_picker_result(ctx, path.as_ref(), *purpose);
            }
        }
    }

    fn execute_dir_picker_result(&mut self, ctx: &Context, path: Option<&PathBuf>, purpose: DirPickerPurpose) {
        match purpose {
            DirPickerPurpose::NewWorkspace { canvas_pos, preset } => {
                let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                let workspace_id = self.create_workspace_at_visible(ctx, &name, canvas_pos);
                super::update_workspace_cwd(self.board.workspace_mut(workspace_id), path);
                let mut options = preset.to_panel_options();
                options.position = Some(canvas_pos);
                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create panel: {error}");
                }
            }
            DirPickerPurpose::AddPanel {
                workspace_id,
                preset,
                canvas_pos,
            } => {
                super::update_workspace_cwd(self.board.workspace_mut(workspace_id), path);
                let mut options = preset.to_panel_options();
                options.position = canvas_pos;
                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create panel: {error}");
                }
            }
        }
        self.mark_runtime_dirty();
    }

    pub(in crate::app) fn handle_canvas_double_click(&mut self, ctx: &Context) {
        let canvas_rect = self.canvas_rect(ctx);
        let ctrl_double_click = ctx.input(|input| {
            let ctrl = input.modifiers.ctrl || input.modifiers.command;
            let double = input.pointer.button_double_clicked(egui::PointerButton::Primary);
            let pos = input.pointer.interact_pos();
            if ctrl && double {
                pos.filter(|pos| canvas_rect.contains(*pos))
            } else {
                None
            }
        });

        let Some(screen_pos) = ctrl_double_click else {
            return;
        };

        let canvas_pos = self.screen_to_canvas(canvas_rect, screen_pos);
        let hit_workspace = self
            .workspace_screen_rects
            .iter()
            .find(|(_, rect)| rect.contains(screen_pos))
            .map(|(id, _)| *id);
        self.pending_preset_pick = Some((hit_workspace, [canvas_pos.x, canvas_pos.y], std::time::Instant::now()));
    }

    pub(in crate::app) fn render_preset_picker(&mut self, ctx: &Context) {
        let Some((target_workspace, canvas_pos, opened_at)) = self.pending_preset_pick else {
            return;
        };

        let popup_id = Id::new("canvas_preset_picker");
        let canvas_rect = self.canvas_rect(ctx);
        let screen_pos = self.canvas_to_screen(canvas_rect, Pos2::new(canvas_pos[0], canvas_pos[1]));
        let (popup_rect, selected_action) =
            self.show_preset_picker_popup(ctx, popup_id, screen_pos, target_workspace, canvas_pos);

        if let Some(action) = selected_action {
            self.pending_preset_pick = None;
            self.apply_preset_picker_action(ctx, action);
        } else if opened_at.elapsed() > std::time::Duration::from_millis(150) {
            let clicked_outside = ctx.input(|input| {
                input.pointer.any_click()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| !popup_rect.contains(pos))
            });
            if clicked_outside {
                self.pending_preset_pick = None;
            }
        }
    }

    fn show_preset_picker_popup(
        &self,
        ctx: &Context,
        popup_id: Id,
        screen_pos: Pos2,
        target_workspace: Option<WorkspaceId>,
        canvas_pos: [f32; 2],
    ) -> (Rect, Option<PresetPickerAction>) {
        let mut selected_action = None;
        let area_response = egui::Area::new(popup_id)
            .fixed_pos(screen_pos)
            .constrain(true)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::PANEL_BG)
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(160.0);
                        ui.label(
                            egui::RichText::new(preset_picker_heading(target_workspace))
                                .size(11.0)
                                .color(theme::FG_DIM)
                                .strong(),
                        );
                        ui.add_space(4.0);

                        if let Some(action) =
                            render_grouped_preset_rows(ui, target_workspace, canvas_pos, &self.presets)
                        {
                            selected_action = Some(action);
                        }
                    });
            });

        (area_response.response.rect, selected_action)
    }

    fn apply_preset_picker_action(&mut self, ctx: &Context, action: PresetPickerAction) {
        match action {
            PresetPickerAction::CreatePanel {
                workspace_id,
                preset,
                canvas_pos,
            } => {
                self.add_panel_to_workspace(workspace_id, preset, canvas_pos);
            }
            PresetPickerAction::ChooseDirectory {
                workspace_id,
                preset,
                canvas_pos,
            } => {
                self.open_panel_dir_picker(workspace_id, preset, canvas_pos);
            }
            PresetPickerAction::CreateWorkspace { canvas_pos, preset } => {
                self.dir_picker = Some(DirPicker::new(DirPickerPurpose::NewWorkspace { canvas_pos, preset }));
            }
            PresetPickerAction::CreateWorkspaceDirect { canvas_pos, preset } => {
                let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                let workspace_id = self.create_workspace_at_visible(ctx, &name, canvas_pos);
                let mut options = preset.to_panel_options();
                options.position = Some(canvas_pos);
                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create panel: {error}");
                }
                self.mark_runtime_dirty();
            }
        }
    }
}
