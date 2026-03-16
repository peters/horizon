use std::cmp::Ordering;
use std::path::PathBuf;

use egui::{Button, Color32, Context, Id, Margin, Order, Pos2, Rect, Stroke, Vec2};
use horizon_core::{PanelId, PanelKind, PanelOptions, PanelTranscript, PresetConfig, WorkspaceId};

use crate::dir_picker::{DirPicker, DirPickerAction, DirPickerPurpose};
use crate::quick_nav::{QuickNav, QuickNavAction, WorkspaceEntry};
use crate::theme;

use super::util::{editor_panel_size_for_file, primary_shortcut_modifier, viewport_local_rect};
use super::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT, WS_BG_PAD, WS_TITLE_HEIGHT};

impl HorizonApp {
    pub(super) fn reset_view(&mut self) {
        self.pan_offset = Vec2::ZERO;
        self.pan_target = None;
        self.mark_runtime_dirty();
    }

    pub(super) fn animate_pan(&mut self, ctx: &Context) {
        if let Some(target) = self.pan_target {
            let dt = ctx.input(|input| input.predicted_dt);
            let t = (20.0 * dt).min(1.0);
            self.pan_offset = self.pan_offset + (target - self.pan_offset) * t;
            if (self.pan_offset - target).length_sq() < 1.0 {
                self.pan_offset = target;
                self.pan_target = None;
            }
            self.mark_runtime_dirty();
        }
    }

    pub(super) fn leftmost_workspace_id(&self) -> Option<WorkspaceId> {
        self.board
            .workspaces
            .iter()
            .min_by(|left, right| {
                left.position[0]
                    .partial_cmp(&right.position[0])
                    .unwrap_or(Ordering::Equal)
            })
            .map(|workspace| workspace.id)
    }

    pub(super) fn pan_to_canvas_pos_aligned(
        &mut self,
        ctx: &Context,
        canvas_pos: Pos2,
        canvas_size: Vec2,
        left_align: bool,
    ) {
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let pan_margin = 40.0;
        let x = if left_align {
            pan_margin - canvas_pos.x
        } else {
            canvas_rect.width() * 0.5 - (canvas_pos.x + canvas_size.x * 0.5)
        };
        let y = canvas_rect.height() * 0.5 - (canvas_pos.y + canvas_size.y * 0.5);

        self.pan_target = Some(Vec2::new(x, y));
    }

    pub(super) fn canvas_to_screen(&self, canvas_rect: Rect, position: Pos2) -> Pos2 {
        canvas_rect.min + self.pan_offset + position.to_vec2()
    }

    pub(super) fn screen_to_canvas(&self, canvas_rect: Rect, screen_pos: Pos2) -> Pos2 {
        Pos2::new(
            screen_pos.x - canvas_rect.min.x - self.pan_offset.x,
            screen_pos.y - canvas_rect.min.y - self.pan_offset.y,
        )
    }

    pub(super) fn canvas_rect(ctx: &Context, sidebar_visible: bool) -> Rect {
        let rect = viewport_local_rect(ctx);
        let left = if sidebar_visible {
            rect.min.x + SIDEBAR_WIDTH
        } else {
            rect.min.x
        };
        Rect::from_min_max(Pos2::new(left, rect.min.y + TOOLBAR_HEIGHT), rect.max)
    }

    pub(super) fn terminal_accepts_keyboard_input(&self, ctx: &Context) -> bool {
        let focused_has_terminal = self
            .board
            .focused
            .and_then(|panel_id| self.board.panel(panel_id))
            .is_some_and(|panel| panel.content.terminal().is_some());
        focused_has_terminal && !ctx.wants_keyboard_input() && self.dir_picker.is_none()
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
        if let Some(transcript) = transcript
            && let Err(error) = transcript.delete_all()
        {
            tracing::warn!(panel_id = panel_id.0, "failed to delete panel transcript: {error}");
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
        let workspace_cwd = self
            .board
            .workspace(workspace_id)
            .and_then(|workspace| workspace.cwd.clone());
        if let Some(cwd) = workspace_cwd {
            let mut options = preset.to_panel_options();
            options.cwd = Some(cwd);
            options.position = canvas_pos;
            if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                tracing::error!("failed to create panel: {error}");
            }
            self.mark_runtime_dirty();
        } else {
            self.dir_picker = Some(DirPicker::new(DirPickerPurpose::AddPanel {
                workspace_id,
                preset,
                canvas_pos,
            }));
        }
    }

    pub(super) fn render_quick_nav(&mut self, ctx: &Context) {
        let Some(nav) = self.quick_nav.as_mut() else {
            return;
        };

        let entries: Vec<WorkspaceEntry> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                WorkspaceEntry {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color: Color32::from_rgb(r, g, b),
                    panel_count: workspace.panels.len(),
                    is_active: self.board.active_workspace == Some(workspace.id),
                }
            })
            .collect();

        match nav.show(ctx, &entries) {
            QuickNavAction::None => {}
            QuickNavAction::Cancelled => self.quick_nav = None,
            QuickNavAction::Selected(workspace_id) => {
                self.quick_nav = None;
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
                self.execute_dir_picker_result(path, purpose);
            }
        }
    }

    fn execute_dir_picker_result(&mut self, path: Option<PathBuf>, purpose: DirPickerPurpose) {
        match purpose {
            DirPickerPurpose::NewWorkspace { canvas_pos, preset } => {
                let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                let workspace_id = self.board.create_workspace_at(&name, canvas_pos);
                if let Some(ref cwd) = path
                    && let Some(workspace) = self.board.workspace_mut(workspace_id)
                {
                    workspace.cwd = Some(cwd.clone());
                }
                let mut options = preset.to_panel_options();
                options.cwd = path;
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
                let mut options = preset.to_panel_options();
                options.cwd = path;
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
            let f11 = input.key_pressed(egui::Key::F11);
            let ctrl = input.modifiers.ctrl || input.modifiers.command;
            (f11 && !ctrl, f11 && ctrl, input.key_pressed(egui::Key::Escape))
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
        if ctx.input(|input| input.key_pressed(egui::Key::K) && primary_shortcut_modifier(input.modifiers)) {
            self.quick_nav = if self.quick_nav.is_some() {
                None
            } else {
                Some(QuickNav::new())
            };
        }

        if self.terminal_accepts_keyboard_input(ctx) {
            return;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Comma) && primary_shortcut_modifier(input.modifiers)) {
            self.toggle_settings();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::B) && primary_shortcut_modifier(input.modifiers)) {
            self.sidebar_visible = !self.sidebar_visible;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::H) && primary_shortcut_modifier(input.modifiers)) {
            self.hud_visible = !self.hud_visible;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::M) && primary_shortcut_modifier(input.modifiers)) {
            self.minimap_visible = !self.minimap_visible;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::N) && primary_shortcut_modifier(input.modifiers)) {
            let workspace_id = self.board.ensure_workspace();
            if let Some(preset) = self.presets.first().cloned() {
                self.add_panel_to_workspace(workspace_id, preset, None);
            } else {
                self.create_panel();
            }
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Num0) && primary_shortcut_modifier(input.modifiers)) {
            self.reset_view();
        }
    }

    pub(super) fn handle_file_drop(&mut self, ctx: &Context) {
        let (hovered, dropped, pointer_pos) = ctx.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input.raw.dropped_files.clone(),
                input.pointer.hover_pos().or(input.pointer.latest_pos()),
            )
        });

        if hovered && let Some(pos) = pointer_pos {
            self.file_hover_pos = Some(pos);
        }

        if dropped.is_empty() {
            return;
        }

        let screen_pos = self.file_hover_pos.or(pointer_pos);
        self.file_hover_pos = None;
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let canvas_pos = screen_pos.map(|pos| self.screen_to_canvas(canvas_rect, pos));

        for file in dropped {
            let Some(path) = file.path else { continue };
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if !matches!(ext, "md" | "markdown" | "txt" | "mdx") {
                continue;
            }

            let workspace_id = self
                .board
                .active_workspace
                .unwrap_or_else(|| self.board.ensure_workspace());
            let options = PanelOptions {
                name: path.file_name().map(|name| name.to_string_lossy().to_string()),
                command: Some(path.display().to_string()),
                kind: PanelKind::Editor,
                position: canvas_pos.map(|pos| [pos.x, pos.y]),
                size: Some(editor_panel_size_for_file(&path)),
                ..PanelOptions::default()
            };
            if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                tracing::error!("failed to create editor panel from dropped file: {error}");
            }
            self.mark_runtime_dirty();
        }
    }

    pub(super) fn handle_canvas_double_click(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
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
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let screen_pos = self.canvas_to_screen(canvas_rect, Pos2::new(canvas_pos[0], canvas_pos[1]));
        let mut open_dir_picker: Option<DirPickerPurpose> = None;

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
                        let heading = if target_workspace.is_some() {
                            "New Terminal"
                        } else {
                            "New Workspace"
                        };
                        ui.label(egui::RichText::new(heading).size(11.0).color(theme::FG_DIM).strong());
                        ui.add_space(4.0);

                        for preset in &self.presets {
                            let label = if let Some(alias) = &preset.alias {
                                format!("{} ({})", preset.name, alias)
                            } else {
                                preset.name.clone()
                            };
                            let text = egui::RichText::new(label).size(12.5).color(theme::FG_SOFT);
                            if ui.add(Button::new(text).frame(false)).clicked() {
                                open_dir_picker = Some(if let Some(workspace_id) = target_workspace {
                                    DirPickerPurpose::AddPanel {
                                        workspace_id,
                                        preset: preset.clone(),
                                        canvas_pos: Some(canvas_pos),
                                    }
                                } else {
                                    DirPickerPurpose::NewWorkspace {
                                        canvas_pos,
                                        preset: preset.clone(),
                                    }
                                });
                            }
                        }
                    });
            });

        if let Some(purpose) = open_dir_picker {
            self.pending_preset_pick = None;
            match purpose {
                DirPickerPurpose::AddPanel {
                    workspace_id,
                    preset,
                    canvas_pos,
                } => {
                    self.add_panel_to_workspace(workspace_id, preset, canvas_pos);
                }
                purpose @ DirPickerPurpose::NewWorkspace { .. } => {
                    self.dir_picker = Some(DirPicker::new(purpose));
                }
            }
        } else if opened_at.elapsed() > std::time::Duration::from_millis(150) {
            let popup_rect = area_response.response.rect;
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

    pub(super) fn handle_canvas_pan(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let panel_rects: Vec<Rect> = self.panel_screen_rects.values().copied().collect();
        let pan_delta = ctx.input(|input| {
            let pointer_position = input.pointer.hover_pos();
            let pointer_in_canvas = pointer_position.is_some_and(|position| canvas_rect.contains(position));
            let pointer_over_panel =
                pointer_position.is_some_and(|position| panel_rects.iter().any(|rect| rect.contains(position)));
            let drag_panning = pointer_in_canvas
                && (input.pointer.middle_down() || (input.key_down(egui::Key::Space) && input.pointer.primary_down()));
            let scroll_panning =
                pointer_in_canvas && !pointer_over_panel && !input.modifiers.ctrl && !input.modifiers.command;

            if drag_panning {
                input.pointer.delta()
            } else if scroll_panning {
                let scroll = input.smooth_scroll_delta + input.raw_scroll_delta;
                if input.modifiers.shift && scroll.x == 0.0 {
                    Vec2::new(scroll.y, 0.0)
                } else {
                    scroll
                }
            } else {
                Vec2::ZERO
            }
        });

        self.is_panning = pan_delta != Vec2::ZERO;
        if self.is_panning {
            self.pan_target = None;
            self.pan_offset += pan_delta;
            self.mark_runtime_dirty();
            // Clear any active terminal text selection so it doesn't
            // continue extending while the canvas is being panned.
            for panel in &self.board.panels {
                if let Some(terminal) = panel.terminal() {
                    terminal.clear_selection();
                }
            }
        }
    }
}
