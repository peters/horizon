use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use egui::containers::panel::PanelState;
use egui::{Context, Id, Margin, Order, Pos2, Rect, Stroke, Vec2};
use horizon_core::{PanelId, PanelOptions, PanelTranscript, PresetConfig, WorkspaceId};

use crate::command_palette::{CommandPalette, PaletteAction};
use crate::command_registry::CommandId;
use crate::dir_picker::{DirPicker, DirPickerAction, DirPickerPurpose};
use crate::theme;

use self::support::{
    command_palette_panel_entries, command_palette_preset_entries, command_palette_workspace_entries,
    detached_workspace_ids, preset_picker_heading, render_preset_picker_row,
};
use super::settings::{SETTINGS_BAR_HEIGHT, SETTINGS_BAR_ID, SETTINGS_PANEL_ID, settings_panel_default_width};
use super::shortcuts::shortcut_pressed;
use super::util::{OverlayExclusion, viewport_local_rect};
use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, SIDEBAR_WIDTH, TOOLBAR_HEIGHT, WS_BG_PAD, WS_TITLE_HEIGHT};

mod support;

fn panel_focus_target_at_pointer_press(
    panel_order: &[PanelId],
    panel_rects: &HashMap<PanelId, Rect>,
    focused_panel: Option<PanelId>,
    pointer_pos: Pos2,
) -> Option<PanelId> {
    if focused_panel.is_some_and(|panel_id| {
        panel_rects
            .get(&panel_id)
            .is_some_and(|rect| rect.contains(pointer_pos))
    }) {
        return focused_panel;
    }

    panel_order
        .iter()
        .rev()
        .copied()
        .find(|panel_id| panel_rects.get(panel_id).is_some_and(|rect| rect.contains(pointer_pos)))
}

impl HorizonApp {
    pub(super) fn leftmost_workspace_id(&self) -> Option<WorkspaceId> {
        self.board
            .workspaces
            .iter()
            .filter(|workspace| !self.workspace_is_detached(workspace.id))
            .min_by(|left, right| {
                left.position[0]
                    .partial_cmp(&right.position[0])
                    .unwrap_or(Ordering::Equal)
            })
            .map(|workspace| workspace.id)
    }

    pub(super) fn canvas_rect(&self, ctx: &Context) -> Rect {
        let viewport = viewport_local_rect(ctx);
        let settings_panel_rect = self.settings_panel_rect(ctx, viewport);
        let settings_bar_rect = self.settings_bar_rect(ctx, viewport);
        canvas_rect_for_layout(viewport, self.sidebar_visible, settings_panel_rect, settings_bar_rect)
    }

    pub(super) fn fixed_overlays_visible(&self) -> bool {
        self.settings.is_none()
    }

    fn settings_panel_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        estimated_settings_panel_rect(
            viewport,
            self.settings.is_some(),
            PanelState::load(ctx, Id::new(SETTINGS_PANEL_ID)).map(|state| state.rect),
        )
    }

    fn settings_bar_rect(&self, ctx: &Context, viewport: Rect) -> Option<Rect> {
        estimated_settings_bar_rect(
            viewport,
            self.settings.is_some(),
            PanelState::load(ctx, Id::new(SETTINGS_BAR_ID)).map(|state| state.rect),
        )
    }

    /// Screen-space rectangles occupied by fixed overlay widgets.  Compute
    /// this once per frame and pass to rendering code that positions
    /// canvas-space elements (e.g. workspace labels) so they stay clear.
    pub(super) fn overlay_exclusion_zones(&self, ctx: &Context) -> OverlayExclusion {
        let viewport = viewport_local_rect(ctx);
        let mut zones = Vec::new();

        if self.sidebar_visible {
            zones.push(Rect::from_min_max(
                Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT),
                Pos2::new(viewport.min.x + SIDEBAR_WIDTH, viewport.max.y),
            ));
        }

        if let Some(rect) = self.settings_panel_rect(ctx, viewport) {
            zones.push(rect);
        }
        if let Some(rect) = self.settings_bar_rect(ctx, viewport) {
            zones.push(rect);
        }

        let minimap_height =
            if self.fixed_overlays_visible() && self.minimap_visible && !self.board.workspaces.is_empty() {
                let overlays = &self.template_config.overlays;
                let w = overlays.minimap_width.max(120.0) + MINIMAP_PAD * 2.0;
                let h = overlays.minimap_height.max(120.0) + MINIMAP_PAD * 2.0;
                zones.push(Rect::from_min_size(
                    Pos2::new(viewport.max.x - MINIMAP_MARGIN - w, viewport.max.y - MINIMAP_MARGIN - h),
                    Vec2::new(w, h),
                ));
                h
            } else {
                0.0
            };

        if self.fixed_overlays_visible()
            && self.template_config.features.attention_feed
            && let Some(rect) = super::attention_feed::estimated_outer_rect(
                viewport,
                minimap_height,
                &self.template_config.overlays,
                &self.board,
            )
        {
            zones.push(rect);
        }

        OverlayExclusion::new(zones)
    }

    pub(super) fn sync_panel_focus_from_pointer_press(&mut self, ctx: &Context) {
        let Some(pointer_pos) = ctx.input(|input| {
            input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        }) else {
            return;
        };

        let panel_order: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| !self.workspace_is_detached(panel.workspace_id))
            .map(|panel| panel.id)
            .collect();

        if let Some(panel_id) =
            panel_focus_target_at_pointer_press(&panel_order, &self.panel_screen_rects, self.board.focused, pointer_pos)
        {
            self.board.focus(panel_id);
        }
    }

    pub(super) fn create_panel(&mut self) {
        let workspace_id = self.board.ensure_workspace();
        if let Err(error) = self.create_panel_with_options(PanelOptions::default(), workspace_id) {
            tracing::error!("failed to create panel: {error}");
        }
    }

    pub(super) fn create_panel_with_options(
        &mut self,
        mut options: PanelOptions,
        workspace_id: WorkspaceId,
    ) -> horizon_core::Result<PanelId> {
        let workspace_cwd = workspace_cwd(&self.board, workspace_id);
        inherit_workspace_cwd(&mut options, workspace_cwd.as_ref());
        self.resolve_panel_launch_binding(&mut options);
        options.transcript_root.clone_from(&self.transcript_root);
        self.board.create_panel(options, workspace_id)
    }

    pub(super) fn close_panel(&mut self, panel_id: PanelId) {
        let transcript = self
            .board
            .panel(panel_id)
            .and_then(|panel| PanelTranscript::for_panel(panel.kind, self.transcript_root.clone(), &panel.local_id));
        self.board.close_panel(panel_id);
        self.terminal_grid_cache.remove(&panel_id);
        self.editor_preview_cache.remove(&panel_id);
        if let Some(transcript) = transcript
            && let Err(error) = transcript.delete_all()
        {
            tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
        }
    }

    pub(super) fn close_workspace_panels(&mut self, workspace_id: WorkspaceId) {
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
            self.terminal_grid_cache.remove(panel_id);
            self.editor_preview_cache.remove(panel_id);
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

    pub(super) fn clear_workspace_rename(&mut self) {
        self.renaming_workspace = None;
        self.rename_buffer.clear();
    }

    pub(super) fn clear_panel_rename(&mut self) {
        self.renaming_panel = None;
        self.panel_rename_buffer.clear();
    }

    pub(super) fn add_panel_to_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    ) {
        if workspace_cwd(&self.board, workspace_id).is_some() || !preset.requires_workspace_cwd() {
            let mut options = preset.to_panel_options();
            options.position = canvas_pos;
            if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                tracing::error!("failed to create panel: {error}");
            }
            self.mark_runtime_dirty();
        } else {
            self.open_panel_dir_picker(workspace_id, preset, canvas_pos);
        }
    }

    pub(super) fn open_panel_dir_picker(
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

    pub(super) fn render_command_palette(&mut self, ctx: &Context) {
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };

        let detached_workspace_ids = detached_workspace_ids(&self.board, &self.detached_workspaces);
        let workspace_entries =
            command_palette_workspace_entries(&self.board, &detached_workspace_ids, self.board.active_workspace);
        let panel_entries = command_palette_panel_entries(&self.board, &detached_workspace_ids);
        let preset_entries = command_palette_preset_entries(&self.presets);

        let action = palette.show(
            ctx,
            &workspace_entries,
            &panel_entries,
            &preset_entries,
            &self.action_commands_cache,
        );
        match action {
            PaletteAction::None => {}
            PaletteAction::Cancelled => self.command_palette = None,
            PaletteAction::Execute(cmd) => {
                self.command_palette = None;
                self.execute_command(ctx, &cmd);
            }
        }
    }

    fn execute_command(&mut self, ctx: &Context, cmd: &CommandId) {
        match *cmd {
            CommandId::SwitchWorkspace(workspace_id) => {
                self.board.focus_workspace(workspace_id);
                if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                    let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                    let size = Vec2::new(
                        max[0] - min[0] + 2.0 * WS_BG_PAD,
                        max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                    );
                    self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
                }
            }
            CommandId::FocusPanel(panel_id) => {
                self.board.focus(panel_id);
                if let Some(ws_id) = self.board.panel(panel_id).map(|p| p.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(ws_id)
                {
                    self.focus_workspace_bounds(ctx, min, max, true);
                }
            }
            CommandId::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            CommandId::ToggleHud => self.hud_visible = !self.hud_visible,
            CommandId::ToggleMinimap => self.minimap_visible = !self.minimap_visible,
            CommandId::ToggleFullscreenWindow => {
                let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
            }
            CommandId::ToggleFullscreenPanel => {
                self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                    None
                } else {
                    self.board.focused
                };
            }
            CommandId::ResetView => self.reset_view(ctx),
            CommandId::ZoomIn => {
                let canvas_rect = self.canvas_rect(ctx);
                let _ = self.zoom_canvas_at(canvas_rect, canvas_rect.center(), self.canvas_view.zoom * 1.1);
            }
            CommandId::ZoomOut => {
                let canvas_rect = self.canvas_rect(ctx);
                let _ = self.zoom_canvas_at(canvas_rect, canvas_rect.center(), self.canvas_view.zoom / 1.1);
            }
            CommandId::AlignWorkspacesHorizontally => {
                if let Some(workspace_id) = align_attached_workspaces(&mut self.board, &self.detached_workspaces)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    self.focus_workspace_bounds(ctx, min, max, true);
                    self.mark_runtime_dirty();
                }
            }
            CommandId::NewPanel => {
                let workspace_id = self.board.ensure_workspace();
                if let Some(preset) = self.presets.first().cloned() {
                    self.add_panel_to_workspace(workspace_id, preset, None);
                } else {
                    self.create_panel();
                }
            }
            CommandId::OpenRemoteHosts => self.toggle_remote_hosts_overlay(),
            CommandId::CreatePanelFromPreset(index) => {
                if let Some(preset) = self.presets.get(index).cloned() {
                    let workspace_id = self
                        .board
                        .active_workspace
                        .unwrap_or_else(|| self.board.ensure_workspace());
                    self.add_panel_to_workspace(workspace_id, preset, None);
                }
            }
            CommandId::ToggleSettings => self.toggle_settings(),
        }
    }

    pub(super) fn render_dir_picker(&mut self, ctx: &Context) {
        let Some(picker) = self.dir_picker.as_mut() else {
            return;
        };

        match picker.show(ctx) {
            DirPickerAction::None => {}
            DirPickerAction::Cancelled => self.dir_picker = None,
            DirPickerAction::Selected(path, purpose) => {
                self.dir_picker = None;
                self.execute_dir_picker_result(path.as_ref(), *purpose);
            }
        }
    }

    fn execute_dir_picker_result(&mut self, path: Option<&PathBuf>, purpose: DirPickerPurpose) {
        match purpose {
            DirPickerPurpose::NewWorkspace { canvas_pos, preset } => {
                let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                let workspace_id = self.board.create_workspace_at(&name, canvas_pos);
                update_workspace_cwd(self.board.workspace_mut(workspace_id), path);
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
                update_workspace_cwd(self.board.workspace_mut(workspace_id), path);
                let mut options = preset.to_panel_options();
                options.position = canvas_pos;
                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create panel: {error}");
                }
            }
        }
        self.mark_runtime_dirty();
    }

    pub(super) fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
        let (f11, ctrl_f11, escape) = ctx.input(|input| {
            (
                shortcut_pressed(input, self.shortcuts.fullscreen_panel),
                shortcut_pressed(input, self.shortcuts.fullscreen_window),
                shortcut_pressed(input, self.shortcuts.exit_fullscreen_panel),
            )
        });

        if ctrl_f11 {
            let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if f11 {
            self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                None
            } else {
                self.board.focused
            };
        } else if escape && self.fullscreen_panel.is_some() {
            self.fullscreen_panel = None;
        }

        if let Some(panel_id) = self.fullscreen_panel
            && self.board.panel(panel_id).is_none()
        {
            self.fullscreen_panel = None;
        }
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &Context) {
        let shortcut_bindings: &[(_, CommandId)] = &[
            (self.shortcuts.reset_view, CommandId::ResetView),
            (self.shortcuts.zoom_in, CommandId::ZoomIn),
            (self.shortcuts.zoom_out, CommandId::ZoomOut),
            (
                self.shortcuts.align_workspaces_horizontally,
                CommandId::AlignWorkspacesHorizontally,
            ),
            (self.shortcuts.toggle_settings, CommandId::ToggleSettings),
            (self.shortcuts.toggle_sidebar, CommandId::ToggleSidebar),
            (self.shortcuts.toggle_hud, CommandId::ToggleHud),
            (self.shortcuts.toggle_minimap, CommandId::ToggleMinimap),
            (self.shortcuts.open_remote_hosts, CommandId::OpenRemoteHosts),
            (self.shortcuts.new_terminal, CommandId::NewPanel),
        ];

        let (toggle_palette, triggered_command) = ctx.input(|input| {
            let palette = shortcut_pressed(input, self.shortcuts.command_palette);
            let command = shortcut_bindings
                .iter()
                .find(|(binding, _)| shortcut_pressed(input, *binding))
                .map(|(_, id)| id.clone());
            (palette, command)
        });

        if toggle_palette {
            self.command_palette = if self.command_palette.is_some() {
                None
            } else {
                Some(CommandPalette::new())
            };
        }
        if let Some(command_id) = triggered_command {
            self.execute_command(ctx, &command_id);
        }
    }

    pub(super) fn handle_canvas_double_click(&mut self, ctx: &Context) {
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

    pub(super) fn render_preset_picker(&mut self, ctx: &Context) {
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
            self.apply_preset_picker_action(action);
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

                        for preset in &self.presets {
                            if let Some(action) = render_preset_picker_row(ui, target_workspace, canvas_pos, preset) {
                                selected_action = Some(action);
                            }
                        }
                    });
            });

        (area_response.response.rect, selected_action)
    }

    fn apply_preset_picker_action(&mut self, action: PresetPickerAction) {
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
                let workspace_id = self.board.create_workspace_at(&name, canvas_pos);
                let mut options = preset.to_panel_options();
                options.position = Some(canvas_pos);
                if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                    tracing::error!("failed to create panel: {error}");
                }
                self.mark_runtime_dirty();
            }
        }
    }

    #[profiling::function]
    pub(super) fn handle_canvas_pan(&mut self, ctx: &Context) {
        let canvas_rect = self.canvas_rect(ctx);
        let (pointer_position, middle_down, primary_down, space_down, modifiers, scroll, pointer_delta, zoom_delta) =
            ctx.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.pointer.middle_down(),
                    input.pointer.primary_down(),
                    input.key_down(egui::Key::Space),
                    input.modifiers,
                    input.smooth_scroll_delta + input.raw_scroll_delta,
                    input.pointer.delta(),
                    input.zoom_delta(),
                )
            });
        let pointer_in_canvas = pointer_position.is_some_and(|position| canvas_rect.contains(position));
        if pointer_in_canvas && (zoom_delta - 1.0).abs() > f32::EPSILON {
            let anchor = pointer_position.unwrap_or_else(|| canvas_rect.center());
            if self.zoom_canvas_at(canvas_rect, anchor, self.canvas_view.zoom * zoom_delta) {
                self.clear_terminal_selections();
            }
            self.is_panning = false;
            return;
        }

        let drag_panning = pointer_in_canvas && (middle_down || (space_down && primary_down));
        let pointer_over_panel = pointer_position.is_some_and(|position| {
            pointer_in_canvas
                && !drag_panning
                && scroll != Vec2::ZERO
                && !modifiers.ctrl
                && !modifiers.command
                && self.panel_screen_rects.values().any(|rect| rect.contains(position))
        });
        let pan_delta = if drag_panning {
            pointer_delta
        } else if pointer_in_canvas && !pointer_over_panel && !modifiers.ctrl && !modifiers.command {
            if modifiers.shift && scroll.x == 0.0 {
                Vec2::new(scroll.y, 0.0)
            } else {
                scroll
            }
        } else {
            Vec2::ZERO
        };

        self.is_panning = pan_delta != Vec2::ZERO;
        if self.is_panning {
            self.pan_target = None;
            let mut pan_offset = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
            pan_offset += pan_delta;
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
            self.mark_runtime_dirty();
            self.clear_terminal_selections();
        }
    }

    fn clear_terminal_selections(&self) {
        for panel in &self.board.panels {
            if let Some(terminal) = panel.terminal() {
                terminal.clear_selection();
            }
        }
    }
}

fn canvas_rect_for_layout(
    viewport: Rect,
    sidebar_visible: bool,
    settings_panel_rect: Option<Rect>,
    settings_bar_rect: Option<Rect>,
) -> Rect {
    let left = if sidebar_visible {
        viewport.min.x + SIDEBAR_WIDTH
    } else {
        viewport.min.x
    };
    let right = settings_panel_rect.map_or(viewport.max.x, |rect| rect.min.x);
    let bottom = settings_bar_rect.map_or(viewport.max.y, |rect| rect.min.y);

    Rect::from_min_max(
        Pos2::new(left, viewport.min.y + TOOLBAR_HEIGHT),
        Pos2::new(right, bottom),
    )
}

fn estimated_settings_panel_rect(viewport: Rect, settings_open: bool, remembered_rect: Option<Rect>) -> Option<Rect> {
    if !settings_open {
        return None;
    }

    remembered_rect.or_else(|| {
        let width = settings_panel_default_width(viewport.width());
        Some(Rect::from_min_max(
            Pos2::new(viewport.max.x - width, viewport.min.y + TOOLBAR_HEIGHT),
            Pos2::new(viewport.max.x, viewport.max.y - SETTINGS_BAR_HEIGHT),
        ))
    })
}

fn estimated_settings_bar_rect(viewport: Rect, settings_open: bool, remembered_rect: Option<Rect>) -> Option<Rect> {
    if !settings_open {
        return None;
    }

    remembered_rect.or_else(|| {
        Some(Rect::from_min_max(
            Pos2::new(viewport.min.x, viewport.max.y - SETTINGS_BAR_HEIGHT),
            viewport.max,
        ))
    })
}

fn workspace_cwd(board: &horizon_core::Board, workspace_id: WorkspaceId) -> Option<PathBuf> {
    board
        .workspace(workspace_id)
        .and_then(|workspace| workspace.cwd.clone())
}

enum PresetPickerAction {
    CreatePanel {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
    ChooseDirectory {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
    CreateWorkspace {
        canvas_pos: [f32; 2],
        preset: PresetConfig,
    },
    CreateWorkspaceDirect {
        canvas_pos: [f32; 2],
        preset: PresetConfig,
    },
}

fn inherit_workspace_cwd(options: &mut PanelOptions, workspace_cwd: Option<&PathBuf>) {
    if options.cwd.is_none()
        && let Some(workspace_cwd) = workspace_cwd
    {
        options.cwd = Some(workspace_cwd.clone());
    }
}

fn update_workspace_cwd(workspace: Option<&mut horizon_core::Workspace>, path: Option<&PathBuf>) {
    if let Some(path) = path
        && let Some(workspace) = workspace
    {
        workspace.cwd = Some(path.clone());
    }
}

fn align_attached_workspaces(
    board: &mut horizon_core::Board,
    detached_workspaces: &BTreeMap<String, horizon_core::WindowConfig>,
) -> Option<WorkspaceId> {
    let detached_workspace_ids = detached_workspace_ids(board, detached_workspaces);
    let workspace_ids: Vec<_> = board
        .workspaces
        .iter()
        .filter(|workspace| !detached_workspace_ids.contains(&workspace.id))
        .map(|workspace| workspace.id)
        .collect();
    board.align_workspaces_horizontally(&workspace_ids)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::path::PathBuf;

    use egui::{Color32, Pos2, Rect};
    use horizon_core::{Board, PanelId, PanelKind, PanelOptions, PresetConfig, WindowConfig, Workspace, WorkspaceId};

    use super::{
        SIDEBAR_WIDTH, TOOLBAR_HEIGHT, align_attached_workspaces, canvas_rect_for_layout,
        command_palette_panel_entries, command_palette_preset_entries, command_palette_workspace_entries,
        detached_workspace_ids, estimated_settings_bar_rect, estimated_settings_panel_rect, inherit_workspace_cwd,
        panel_focus_target_at_pointer_press, update_workspace_cwd, workspace_cwd,
    };
    use crate::app::settings::SETTINGS_BAR_HEIGHT;

    #[test]
    fn inherit_workspace_cwd_populates_missing_panel_cwd() {
        let mut options = PanelOptions::default();
        let workspace_path = PathBuf::from("/repo");

        inherit_workspace_cwd(&mut options, Some(&workspace_path));

        assert_eq!(options.cwd, Some(workspace_path));
    }

    #[test]
    fn inherit_workspace_cwd_preserves_explicit_panel_cwd() {
        let panel_path = PathBuf::from("/panel");
        let workspace_path = PathBuf::from("/repo");
        let mut options = PanelOptions {
            cwd: Some(panel_path.clone()),
            ..PanelOptions::default()
        };

        inherit_workspace_cwd(&mut options, Some(&workspace_path));

        assert_eq!(options.cwd, Some(panel_path));
    }

    #[test]
    fn update_workspace_cwd_promotes_selected_panel_directory() {
        let mut workspace = Workspace::new(WorkspaceId(1), "alpha".to_string(), 0);
        let selected_path = PathBuf::from("/repo");

        update_workspace_cwd(Some(&mut workspace), Some(&selected_path));

        assert_eq!(workspace.cwd, Some(selected_path));
    }

    #[test]
    fn update_workspace_cwd_keeps_existing_directory_when_picker_is_skipped() {
        let existing_path = PathBuf::from("/repo");
        let mut workspace = Workspace::new(WorkspaceId(1), "alpha".to_string(), 0);
        workspace.cwd = Some(existing_path.clone());

        update_workspace_cwd(Some(&mut workspace), None);

        assert_eq!(workspace.cwd, Some(existing_path));
    }

    #[test]
    fn workspace_cwd_reads_workspace_default_directory() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("alpha");
        let path = PathBuf::from("/repo");
        board.workspace_mut(workspace_id).expect("workspace").cwd = Some(path.clone());

        assert_eq!(workspace_cwd(&board, workspace_id), Some(path));
    }

    #[test]
    fn command_palette_preset_entries_include_ssh_keywords() {
        let presets = vec![PresetConfig {
            name: "SSH: prod-api".to_string(),
            alias: Some("pa".to_string()),
            kind: PanelKind::Ssh,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: Some(horizon_core::SshConnection {
                host: "prod-api".to_string(),
                user: Some("deploy".to_string()),
                ..horizon_core::SshConnection::default()
            }),
        }];

        let entries = command_palette_preset_entries(&presets);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail, "deploy@prod-api");
        assert!(entries[0].keywords.iter().any(|keyword| keyword == "deploy"));
    }

    #[test]
    fn estimated_settings_panel_rect_uses_default_wide_fallback() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));

        let rect = estimated_settings_panel_rect(viewport, true, None).expect("settings rect");

        assert_eq!(rect.min, Pos2::new(840.0, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(1200.0, 752.0));
    }

    #[test]
    fn estimated_settings_panel_rect_clamps_narrow_fallback_width() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(700.0, 800.0));

        let rect = estimated_settings_panel_rect(viewport, true, None).expect("settings rect");

        assert_eq!(rect.min, Pos2::new(360.0, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(700.0, 752.0));
    }

    #[test]
    fn estimated_settings_panel_rect_prefers_remembered_panel_state() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));
        let remembered = Rect::from_min_max(Pos2::new(900.0, 60.0), Pos2::new(1200.0, 720.0));

        let rect = estimated_settings_panel_rect(viewport, true, Some(remembered)).expect("settings rect");

        assert_eq!(rect, remembered);
    }

    #[test]
    fn estimated_settings_rects_close_when_settings_are_hidden() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));

        assert_eq!(estimated_settings_panel_rect(viewport, false, None), None);
        assert_eq!(estimated_settings_bar_rect(viewport, false, None), None);
    }

    #[test]
    fn canvas_rect_for_layout_excludes_sidebar_settings_panel_and_bar() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));
        let settings_panel = Rect::from_min_max(Pos2::new(840.0, TOOLBAR_HEIGHT), Pos2::new(1200.0, 752.0));
        let settings_bar = Rect::from_min_max(Pos2::new(0.0, 800.0 - SETTINGS_BAR_HEIGHT), Pos2::new(1200.0, 800.0));

        let rect = canvas_rect_for_layout(viewport, true, Some(settings_panel), Some(settings_bar));

        assert_eq!(rect.min, Pos2::new(SIDEBAR_WIDTH, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(840.0, 800.0 - SETTINGS_BAR_HEIGHT));
    }

    #[test]
    fn detached_workspace_ids_resolve_from_local_ids() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");
        let detached_local_id = board.workspace(detached).expect("detached workspace").local_id.clone();

        let mut detached_workspaces = BTreeMap::new();
        detached_workspaces.insert(detached_local_id, WindowConfig::default());

        let ids = detached_workspace_ids(&board, &detached_workspaces);

        assert!(ids.contains(&detached));
        assert!(!ids.contains(&attached));
    }

    #[test]
    fn command_palette_workspace_entries_skip_detached_workspaces() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");

        let entries = command_palette_workspace_entries(&board, &HashSet::from([detached]), Some(attached));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, attached);
        assert_eq!(entries[0].name, "attached");
        assert_eq!(entries[0].color, Color32::from_rgb(137, 180, 250));
        assert!(entries[0].is_active);
    }

    #[test]
    fn command_palette_panel_entries_skip_panels_in_detached_workspaces() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");
        let attached_panel = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Editor,
                    command: Some("attached.md".to_string()),
                    ..PanelOptions::default()
                },
                attached,
            )
            .expect("attached panel");
        board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Editor,
                    command: Some("detached.md".to_string()),
                    ..PanelOptions::default()
                },
                detached,
            )
            .expect("detached panel");

        let entries = command_palette_panel_entries(&board, &HashSet::from([detached]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, attached_panel);
        assert_eq!(entries[0].workspace_name, "attached");
    }

    #[test]
    fn align_attached_workspaces_ignores_detached_workspaces() {
        let mut board = Board::new();
        let left = board.create_workspace("left");
        let detached = board.create_workspace("detached");
        let right = board.create_workspace("right");
        board.move_workspace(left, [100.0, 200.0]);
        board.move_workspace(detached, [300.0, 50.0]);
        board.move_workspace(right, [500.0, 400.0]);

        let detached_local_id = board.workspace(detached).expect("detached workspace").local_id.clone();
        let detached_position = board.workspace(detached).expect("detached workspace").position;
        let detached_workspaces = BTreeMap::from([(detached_local_id, WindowConfig::default())]);

        let leftmost = align_attached_workspaces(&mut board, &detached_workspaces);

        assert_eq!(leftmost, Some(left));
        assert!(board.workspace(detached).is_some_and(|workspace| {
            workspace
                .position
                .iter()
                .zip(detached_position)
                .all(|(current, original)| (current - original).abs() <= f32::EPSILON)
        }));
    }

    #[test]
    fn panel_focus_target_prefers_existing_focused_panel_when_rects_overlap() {
        let panel_a = PanelId(1);
        let panel_b = PanelId(2);
        let panel_rects = HashMap::from([
            (
                panel_a,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
            (
                panel_b,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
        ]);

        let target = panel_focus_target_at_pointer_press(
            &[panel_a, panel_b],
            &panel_rects,
            Some(panel_a),
            Pos2::new(40.0, 40.0),
        );

        assert_eq!(target, Some(panel_a));
    }

    #[test]
    fn panel_focus_target_uses_frontmost_panel_order_for_unfocused_overlap() {
        let panel_a = PanelId(1);
        let panel_b = PanelId(2);
        let panel_rects = HashMap::from([
            (
                panel_a,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
            (
                panel_b,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
        ]);

        let target =
            panel_focus_target_at_pointer_press(&[panel_a, panel_b], &panel_rects, None, Pos2::new(40.0, 40.0));

        assert_eq!(target, Some(panel_b));
    }
}
