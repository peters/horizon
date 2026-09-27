use std::path::PathBuf;

use egui::{Context, Id, Margin, Order, Pos2, Rect, Stroke};
use horizon_core::WorkspaceId;

use crate::app::HorizonApp;
use crate::dir_picker::{DirPicker, DirPickerAction, DirPickerPurpose};
use crate::theme;

use super::PresetPickerAction;
use super::support::{preset_picker_heading, render_grouped_preset_rows};

impl HorizonApp {
    pub(in crate::app) fn preset_picker_rect(&self, ctx: &Context) -> Option<Rect> {
        self.pending_preset_pick?;
        ctx.memory(|memory| memory.area_rect(Id::new("canvas_preset_picker")))
    }

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
                let mut options = preset.to_panel_options(&self.template_config.browser);
                options.position = Some(canvas_pos);
                match self.create_panel_with_options(options, workspace_id) {
                    Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
                    Err(error) => tracing::error!("failed to create panel: {error}"),
                }
            }
            DirPickerPurpose::AddPanel {
                workspace_id,
                preset,
                canvas_pos,
            } => {
                super::update_workspace_cwd(self.board.workspace_mut(workspace_id), path);
                let mut options = preset.to_panel_options(&self.template_config.browser);
                options.position = super::add_panel_position(&self.board, workspace_id, canvas_pos);
                match self.create_panel_with_options(options, workspace_id) {
                    Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
                    Err(error) => tracing::error!("failed to create panel: {error}"),
                }
            }
            #[cfg(feature = "cloud-workspaces")]
            DirPickerPurpose::CloudRepository => {
                if let Some(path) = path {
                    self.set_cloud_repository(path);
                }
                return;
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

        // The minimap and the other fixed overlays float above the canvas and
        // consume their own double-clicks (fit-to-target). Without this the
        // preset picker opens underneath them too — the same reason raw
        // panel-focus handling consults `overlay_exclusion_zones`.
        if self.overlay_exclusion_zones(ctx).contains(screen_pos) {
            return;
        }

        let canvas_pos = self.screen_to_canvas(canvas_rect, screen_pos);
        let hit_workspace = self
            .workspace_screen_rects
            .iter()
            .find(|(_, rect)| rect.contains(screen_pos))
            .map(|(id, _)| *id);
        let target = (hit_workspace, [canvas_pos.x, canvas_pos.y]);
        #[cfg(feature = "cloud-workspaces")]
        let target = self
            .fullscreen_cloud_target()
            .or_else(|| {
                self.cloud_prototype.groups.0.iter().find_map(|group| {
                    let (min, max) = group.bounds();
                    (!group.collapsed && egui::Rect::from_min_max(min.into(), max.into()).contains(canvas_pos))
                        .then(|| self.board.workspace_id_by_local_id(&group.workspace))
                        .flatten()
                        .map(|workspace| (workspace, [canvas_pos.x, canvas_pos.y]))
                })
            })
            .map_or(target, |(workspace, position)| (Some(workspace), position));
        self.pending_preset_pick = Some((target.0, target.1, std::time::Instant::now()));
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
        #[cfg(feature = "cloud-workspaces")]
        let cloud = target_workspace
            .and_then(|workspace| {
                self.cloud_prototype
                    .groups
                    .at_position(&self.board, workspace, canvas_pos)
            })
            .map(|index| &self.cloud_prototype.groups.0[index]);
        let unavailable_reason = |kind| {
            #[cfg(feature = "cloud-workspaces")]
            {
                cloud.and_then(|group| group.unavailable_panel_reason(kind))
            }
            #[cfg(not(feature = "cloud-workspaces"))]
            {
                let _ = kind;
                None
            }
        };
        let mut selected_action = None;
        let area_response = egui::Area::new(popup_id)
            .fixed_pos(screen_pos)
            .constrain(true)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::PANEL_BG())
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_STRONG()))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(160.0);
                        ui.set_max_width(320.0);
                        ui.label(
                            egui::RichText::new(preset_picker_heading(target_workspace))
                                .size(11.0)
                                .color(theme::FG_DIM())
                                .strong(),
                        );
                        ui.add_space(4.0);
                        #[cfg(feature = "cloud-workspaces")]
                        if let Some(workspace_id) = target_workspace {
                            if ui
                                .add_enabled(self.cloud_launch_ready(), egui::Button::new("Cloud").frame(false))
                                .clicked()
                            {
                                selected_action = Some(PresetPickerAction::CreateCloud { workspace_id });
                            }
                            ui.separator();
                        }

                        let action = egui::ScrollArea::vertical()
                            .max_height((ctx.content_rect().height() - 120.0).max(100.0))
                            .show(ui, |ui| {
                                render_grouped_preset_rows(
                                    ui,
                                    target_workspace,
                                    canvas_pos,
                                    &self.presets,
                                    unavailable_reason,
                                )
                            })
                            .inner;
                        if let Some(action) = action {
                            selected_action = Some(action);
                        }
                    });
            });

        (area_response.response.rect, selected_action)
    }

    fn apply_preset_picker_action(&mut self, ctx: &Context, action: PresetPickerAction) {
        match action {
            #[cfg(feature = "cloud-workspaces")]
            PresetPickerAction::CreateCloud { workspace_id } => self.open_cloud_for_workspace(ctx, workspace_id),
            PresetPickerAction::CreatePanel {
                workspace_id,
                preset,
                canvas_pos,
            } => {
                self.add_panel_to_workspace(ctx, workspace_id, preset, canvas_pos);
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
                let mut options = preset.to_panel_options(&self.template_config.browser);
                options.position = Some(canvas_pos);
                match self.create_panel_with_options(options, workspace_id) {
                    Ok(panel_id) => self.reveal_new_panel(ctx, workspace_id, panel_id),
                    Err(error) => tracing::error!("failed to create panel: {error}"),
                }
                self.mark_runtime_dirty();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::test_support::test_app, test_egui::DiscardTextures};

    #[test]
    fn picker_wheel_stays_available_to_the_menu_without_panning_canvas() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace("Fixture");
        app.pending_preset_pick = Some((Some(workspace), [400.0, 200.0], std::time::Instant::now()));
        let ctx = Context::default();
        for _ in 0..2 {
            let _ = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    app.show_preset_picker_popup(
                        ui.ctx(),
                        Id::new("canvas_preset_picker"),
                        Pos2::new(400.0, 200.0),
                        Some(workspace),
                        [400.0, 200.0],
                    );
                })
                .discard_textures();
        }
        let position = app.preset_picker_rect(&ctx).unwrap().center();
        assert!(app.overlay_exclusion_zones(&ctx).contains(position));
        let before = app.canvas_view;
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -80.0),
                            modifiers: egui::Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    app.handle_canvas_pan(ui.ctx());
                    assert!(ui.ctx().input(|input| input.smooth_scroll_delta.y < 0.0));
                },
            )
            .discard_textures();
        assert_eq!(app.canvas_view, before);
        app.pending_preset_pick = None;
        assert!(app.preset_picker_rect(&ctx).is_none());
    }
}
