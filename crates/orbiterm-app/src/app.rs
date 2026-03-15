use std::collections::HashMap;
use std::path::PathBuf;

use egui::{
    Align, Button, Color32, Context, CornerRadius, CursorIcon, Id, Layout, Margin, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, UiBuilder, Vec2,
};
use orbiterm_core::{Board, Config, PanelId, PanelOptions, WorkspaceId};

use crate::terminal_widget::TerminalView;
use crate::theme;

const TOOLBAR_HEIGHT: f32 = 46.0;
const PANEL_TITLEBAR_HEIGHT: f32 = 34.0;
const PANEL_PADDING: f32 = 8.0;
const PANEL_MIN_SIZE: [f32; 2] = [320.0, 220.0];
#[cfg(test)]
const PANEL_COLUMN_SPACING: f32 = 540.0;
#[cfg(test)]
const PANEL_ROW_SPACING: f32 = 360.0;
const RESIZE_HANDLE_SIZE: f32 = 18.0;
const WS_BG_PAD: f32 = 16.0;
const WS_TITLE_HEIGHT: f32 = 38.0;
const WS_EMPTY_SIZE: [f32; 2] = [304.0, 154.0];
const WS_LABEL_HEIGHT: f32 = 30.0;
const WS_LABEL_MIN_WIDTH: f32 = 110.0;
const WS_LABEL_MAX_WIDTH: f32 = 260.0;

struct WorkspaceVisual {
    id: WorkspaceId,
    name: String,
    color: Color32,
    screen_rect: Rect,
    label_rect: Rect,
    is_active: bool,
    is_empty: bool,
}

struct WorkspaceInteraction {
    activate_workspace: bool,
    drag_delta: Vec2,
    start_rename: bool,
    finish_rename: bool,
}

pub struct OrbitermApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    workspace_assignments: Vec<(PanelId, WorkspaceId)>,
    workspace_creates: Vec<PanelId>,
    theme_applied: bool,
    pan_offset: Vec2,
    panel_screen_rects: HashMap<PanelId, Rect>,
    workspace_screen_rects: Vec<(WorkspaceId, Rect)>,
    fullscreen_panel: Option<PanelId>,
    renaming_workspace: Option<WorkspaceId>,
    rename_buffer: String,
}

impl OrbitermApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, config: &Config, _config_path: Option<PathBuf>) -> Self {
        let board = Board::from_config(config).unwrap_or_else(|error| {
            tracing::error!("failed to load config: {error}");
            Board::new()
        });

        Self {
            board,
            panels_to_close: Vec::new(),
            workspace_assignments: Vec::new(),
            workspace_creates: Vec::new(),
            theme_applied: false,
            pan_offset: Vec2::ZERO,
            panel_screen_rects: HashMap::new(),
            workspace_screen_rects: Vec::new(),
            fullscreen_panel: None,
            renaming_workspace: None,
            rename_buffer: String::new(),
        }
    }

    fn reset_view(&mut self) {
        self.pan_offset = Vec2::ZERO;
    }

    fn canvas_to_screen(&self, canvas_rect: Rect, position: Pos2) -> Pos2 {
        canvas_rect.min + self.pan_offset + position.to_vec2()
    }

    fn screen_to_canvas(&self, canvas_rect: Rect, screen_pos: Pos2) -> Pos2 {
        Pos2::new(
            screen_pos.x - canvas_rect.min.x - self.pan_offset.x,
            screen_pos.y - canvas_rect.min.y - self.pan_offset.y,
        )
    }

    fn canvas_rect(ctx: &Context) -> Rect {
        let rect = viewport_local_rect(ctx);
        Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + TOOLBAR_HEIGHT), rect.max)
    }

    fn terminal_accepts_keyboard_input(&self, ctx: &Context) -> bool {
        self.board.focused.is_some() && !ctx.wants_keyboard_input()
    }

    fn create_panel(&mut self) {
        let ws_id = self.board.ensure_workspace();
        if let Err(error) = self.board.create_panel(PanelOptions::default(), ws_id) {
            tracing::error!("failed to create panel: {error}");
        }
    }

    fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
        let (f11, ctrl_f11, escape) = ctx.input(|input| {
            let f11 = input.key_pressed(egui::Key::F11);
            let ctrl = input.modifiers.ctrl || input.modifiers.command;
            (f11 && !ctrl, f11 && ctrl, input.key_pressed(egui::Key::Escape))
        });

        if ctrl_f11 {
            let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if f11 {
            if self.fullscreen_panel.is_some() {
                self.fullscreen_panel = None;
            } else {
                self.fullscreen_panel = self.board.focused;
            }
        } else if escape && self.fullscreen_panel.is_some() {
            self.fullscreen_panel = None;
        }

        // Clear fullscreen if the panel no longer exists.
        if let Some(panel_id) = self.fullscreen_panel
            && self.board.panel(panel_id).is_none()
        {
            self.fullscreen_panel = None;
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        if self.terminal_accepts_keyboard_input(ctx) {
            return;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::N) && input.modifiers.ctrl) {
            self.create_panel();
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Num0) && input.modifiers.ctrl) {
            self.reset_view();
        }
    }

    fn handle_canvas_double_click(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx);
        let ctrl_double_click = ctx.input(|input| {
            let ctrl = input.modifiers.ctrl || input.modifiers.command;
            let double = input
                .pointer
                .button_double_clicked(egui::PointerButton::Primary);
            let pos = input.pointer.interact_pos();
            if ctrl && double {
                pos.filter(|p| canvas_rect.contains(*p))
            } else {
                None
            }
        });

        let Some(screen_pos) = ctrl_double_click else {
            return;
        };

        // Check if click is inside any workspace
        let hit_workspace = self
            .workspace_screen_rects
            .iter()
            .find(|(_, rect)| rect.contains(screen_pos))
            .map(|(id, _)| *id);

        if let Some(ws_id) = hit_workspace {
            // Inside a workspace: create a new panel there
            let canvas_pos = self.screen_to_canvas(canvas_rect, screen_pos);
            let opts = PanelOptions {
                position: Some([canvas_pos.x, canvas_pos.y]),
                ..PanelOptions::default()
            };
            if let Err(error) = self.board.create_panel(opts, ws_id) {
                tracing::error!("failed to create panel: {error}");
            }
        } else {
            // Outside all workspaces: create a new workspace at click position
            let canvas_pos = self.screen_to_canvas(canvas_rect, screen_pos);
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let ws_id = self.board.create_workspace_at(&name, [canvas_pos.x, canvas_pos.y]);
            if let Err(error) = self.board.create_panel(PanelOptions::default(), ws_id) {
                tracing::error!("failed to create panel: {error}");
            }
        }
    }

    fn handle_canvas_pan(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx);
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
                input.smooth_scroll_delta + input.raw_scroll_delta
            } else {
                Vec2::ZERO
            }
        });

        if pan_delta != Vec2::ZERO {
            self.pan_offset += pan_delta;
        }
    }

    fn render_toolbar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(TOOLBAR_HEIGHT)
            .frame(
                egui::Frame::default()
                    .fill(theme::TITLEBAR_BG)
                    .inner_margin(Margin::symmetric(14, 8))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 170))),
            )
            .show(ctx, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::branding::APP_NAME)
                            .color(theme::FG)
                            .size(14.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(crate::branding::APP_TAGLINE)
                            .color(theme::FG_DIM)
                            .size(10.5),
                    );
                    ui.add_space(10.0);
                    if ui.add(primary_button("New Terminal")).clicked() {
                        self.create_panel();
                    }
                    if ui.add(chrome_button("Reset View")).clicked() {
                        self.reset_view();
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("{} panels", self.board.panels.len()))
                            .color(theme::FG_DIM)
                            .size(10.5),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Middle-drag or scroll empty space to pan")
                            .color(theme::FG_DIM)
                            .size(10.5),
                    );

                    ui.separator();

                    let ws_info: Vec<_> = self
                        .board
                        .workspaces
                        .iter()
                        .map(|ws| (ws.id, ws.name.clone(), ws.accent()))
                        .collect();

                    for (ws_id, ws_name, (r, g, b)) in ws_info {
                        let ws_color = Color32::from_rgb(r, g, b);
                        let is_active = self.board.active_workspace == Some(ws_id);
                        let chip = Button::new(egui::RichText::new(&ws_name).size(10.5).color(if is_active {
                            theme::FG
                        } else {
                            theme::FG_SOFT
                        }))
                        .fill(theme::blend(
                            theme::PANEL_BG_ALT,
                            ws_color,
                            if is_active { 0.28 } else { 0.14 },
                        ))
                        .stroke(Stroke::new(
                            1.0,
                            theme::alpha(ws_color, if is_active { 200 } else { 120 }),
                        ))
                        .corner_radius(10);

                        if ui.add(chip).clicked() {
                            self.board.focus_workspace(ws_id);
                        }
                    }

                    if ui.add(chrome_button("+ Workspace")).clicked() {
                        let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                        let _ = self.board.create_workspace(&name);
                    }
                });
            });
    }

    fn render_canvas_hud(&self, ctx: &Context) {
        let view_origin = Pos2::new(-self.pan_offset.x, -self.pan_offset.y);
        let focused_status = self
            .board
            .focused
            .and_then(|panel_id| self.board.panel(panel_id))
            .map_or_else(
                || "none".to_string(),
                |panel| {
                    format!(
                        "{}  {} x {}",
                        format_grid_position(Pos2::new(panel.layout.position[0], panel.layout.position[1])),
                        rounded_i32(panel.layout.size[0]),
                        rounded_i32(panel.layout.size[1]),
                    )
                },
            );

        egui::Area::new(Id::new("canvas_hud"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(16.0, -16.0))
            .interactable(false)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::alpha(theme::PANEL_BG, 236))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_STRONG, 210)))
                    .corner_radius(12)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Canvas HUD").color(theme::FG).size(11.5).strong());
                        ui.label(
                            egui::RichText::new(format!("view origin  {}", format_grid_position(view_origin)))
                                .monospace()
                                .color(theme::FG_SOFT)
                                .size(11.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("focused term {focused_status}"))
                                .monospace()
                                .color(theme::FG_SOFT)
                                .size(11.0),
                        );
                    });
            });
    }

    fn render_fullscreen_panel(&mut self, ctx: &Context) {
        let Some(panel_id) = self.fullscreen_panel else {
            return;
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::PANEL_BG))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let body_rect = Rect::from_min_max(
                    Pos2::new(rect.min.x + PANEL_PADDING, rect.min.y + PANEL_PADDING),
                    Pos2::new(rect.max.x - PANEL_PADDING, rect.max.y - PANEL_PADDING),
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        if let Some(panel) = self.board.panel_mut(panel_id) {
                            TerminalView::new(panel).show(ui, true);
                        }
                    },
                );
            });
    }

    fn render_canvas(&self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                paint_canvas_glow(ui);
                draw_dot_grid(ui, self.pan_offset);

                if self.board.panels.is_empty() {
                    paint_empty_state(ui);
                }
            });
    }

    fn render_panels(&mut self, ctx: &Context) {
        self.panel_screen_rects.clear();

        let workspaces: Vec<(WorkspaceId, String, Color32)> = self
            .board
            .workspaces
            .iter()
            .map(|ws| {
                let (r, g, b) = ws.accent();
                (ws.id, ws.name.clone(), Color32::from_rgb(r, g, b))
            })
            .collect();

        let mut panel_order: Vec<_> = self
            .board
            .panels
            .iter()
            .enumerate()
            .map(|(index, panel)| (panel.id, panel.title.clone(), index))
            .collect();
        let focused = self.board.focused;
        panel_order.sort_by_key(|(panel_id, _, _)| Some(*panel_id) == focused);

        let canvas_rect = Self::canvas_rect(ctx);
        let mut panels_to_close = Vec::new();

        for (panel_id, title, fallback_index) in panel_order {
            if self.render_panel(ctx, canvas_rect, panel_id, &title, fallback_index, &workspaces) {
                panels_to_close.push(panel_id);
            }
        }

        self.panels_to_close = panels_to_close;
    }

    #[allow(clippy::too_many_lines)]
    fn render_panel(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        panel_id: PanelId,
        title: &str,
        _fallback_index: usize,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> bool {
        let Some((canvas_position, canvas_size, current_ws_id)) = self.board.panel(panel_id).map(|panel| {
            (
                Pos2::new(panel.layout.position[0], panel.layout.position[1]),
                Vec2::new(panel.layout.size[0], panel.layout.size[1]),
                panel.workspace_id,
            )
        }) else {
            return false;
        };

        let ws_accent = workspaces
            .iter()
            .find(|(id, _, _)| *id == current_ws_id)
            .map(|(_, _, color)| *color);

        let screen_rect = Rect::from_min_size(self.canvas_to_screen(canvas_rect, canvas_position), canvas_size);
        let is_focused = self.board.focused == Some(panel_id);
        let mut clicked_terminal = false;
        let mut focus_panel = false;
        let mut close_panel = false;
        let mut drag_delta = Vec2::ZERO;
        let mut resize_delta = Vec2::ZERO;
        let mut ws_assign: Option<WorkspaceId> = None;
        let mut ws_create = false;

        egui::Area::new(Id::new(("panel", panel_id.0)))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(if is_focused { Order::Foreground } else { Order::Middle })
            .show(ctx, |ui| {
                let (panel_rect, _) = ui.allocate_exact_size(screen_rect.size(), Sense::hover());
                let titlebar_rect = Rect::from_min_max(
                    panel_rect.min,
                    Pos2::new(panel_rect.max.x, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT),
                );
                let body_rect = Rect::from_min_max(
                    Pos2::new(panel_rect.min.x + PANEL_PADDING, titlebar_rect.max.y + PANEL_PADDING),
                    Pos2::new(panel_rect.max.x - PANEL_PADDING, panel_rect.max.y - PANEL_PADDING),
                );
                let close_rect = Rect::from_center_size(
                    Pos2::new(panel_rect.max.x - 18.0, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT * 0.5),
                    Vec2::splat(16.0),
                );
                let resize_rect = Rect::from_min_size(
                    Pos2::new(
                        panel_rect.max.x - RESIZE_HANDLE_SIZE,
                        panel_rect.max.y - RESIZE_HANDLE_SIZE,
                    ),
                    Vec2::splat(RESIZE_HANDLE_SIZE),
                );

                let drag_response = ui.interact(
                    titlebar_rect,
                    ui.make_persistent_id(("panel_drag", panel_id.0)),
                    Sense::click_and_drag(),
                );
                let close_response = ui.interact(
                    close_rect.expand2(Vec2::splat(4.0)),
                    ui.make_persistent_id(("panel_close", panel_id.0)),
                    Sense::click(),
                );
                let resize_response = ui.interact(
                    resize_rect.expand2(Vec2::splat(6.0)),
                    ui.make_persistent_id(("panel_resize", panel_id.0)),
                    Sense::click_and_drag(),
                );

                if drag_response.clicked()
                    || drag_response.drag_started()
                    || resize_response.drag_started()
                    || resize_response.clicked()
                {
                    focus_panel = true;
                }
                if drag_response.dragged() {
                    drag_delta = ctx.input(|input| input.pointer.delta());
                }
                if resize_response.dragged() {
                    resize_delta = ctx.input(|input| input.pointer.delta());
                }
                if close_response.clicked() {
                    close_panel = true;
                }

                drag_response.context_menu(|ui| {
                    ui.set_min_width(180.0);
                    ui.label(egui::RichText::new("Move to Workspace").size(11.0).color(theme::FG_DIM));
                    ui.separator();

                    for (ws_id, ws_name, ws_color) in workspaces {
                        let is_current = current_ws_id == *ws_id;
                        let label = if is_current {
                            format!("● {ws_name}")
                        } else {
                            format!("  {ws_name}")
                        };
                        let text = egui::RichText::new(label)
                            .color(if is_current { *ws_color } else { theme::FG_SOFT })
                            .size(12.0);
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            ws_assign = Some(*ws_id);
                            ui.close();
                        }
                    }

                    ui.separator();
                    if ui.button("New Workspace").clicked() {
                        ws_create = true;
                        ui.close();
                    }
                });

                paint_panel_chrome(
                    ui,
                    panel_rect,
                    titlebar_rect,
                    close_rect,
                    resize_rect,
                    title,
                    is_focused,
                    close_response.hovered(),
                    ws_accent,
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        if let Some(panel) = self.board.panel_mut(panel_id) {
                            clicked_terminal = TerminalView::new(panel).show(ui, is_focused);
                        }
                    },
                );
            });

        self.panel_screen_rects.insert(panel_id, screen_rect);

        if drag_delta != Vec2::ZERO {
            let new_position = canvas_position + drag_delta;
            let _ = self.board.move_panel(panel_id, [new_position.x, new_position.y]);
        }

        if resize_delta != Vec2::ZERO {
            let new_size = clamp_panel_size(canvas_size + resize_delta);
            let _ = self.board.resize_panel(panel_id, [new_size.x, new_size.y]);
        }

        if clicked_terminal || focus_panel {
            self.board.focus(panel_id);
        }

        if ws_create {
            self.workspace_creates.push(panel_id);
        }
        if let Some(ws_id) = ws_assign {
            self.workspace_assignments.push((panel_id, ws_id));
        }

        close_panel
    }

    fn render_workspace_backgrounds(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_rect(ctx);
        let visuals = self.workspace_visuals(canvas_rect);

        self.workspace_screen_rects.clear();
        let mut pending_workspace_moves = Vec::new();
        let mut focus_workspace = None;
        let mut start_rename_ws = None;
        let mut finish_rename = false;

        for workspace in &visuals {
            self.workspace_screen_rects
                .push((workspace.id, workspace.screen_rect));

            let is_renaming = self.renaming_workspace == Some(workspace.id);
            let interaction = if is_renaming {
                render_workspace_visual(ctx, workspace, Some(&mut self.rename_buffer))
            } else {
                render_workspace_visual(ctx, workspace, None)
            };

            if interaction.activate_workspace {
                focus_workspace = Some(workspace.id);
            }

            if interaction.drag_delta != Vec2::ZERO {
                pending_workspace_moves.push((workspace.id, interaction.drag_delta));
            }

            if interaction.start_rename {
                start_rename_ws = Some((workspace.id, workspace.name.clone()));
            }

            if interaction.finish_rename {
                finish_rename = true;
            }
        }

        if let Some((ws_id, current_name)) = start_rename_ws {
            self.renaming_workspace = Some(ws_id);
            self.rename_buffer = current_name;
        }

        if finish_rename
            && let Some(ws_id) = self.renaming_workspace.take()
        {
            let name = self.rename_buffer.trim().to_string();
            if !name.is_empty() {
                let _ = self.board.rename_workspace(ws_id, &name);
            }
            self.rename_buffer.clear();
        }

        if let Some(workspace_id) = focus_workspace {
            self.board.focus_workspace(workspace_id);
        }

        for (workspace_id, delta) in pending_workspace_moves {
            let _ = self.board.translate_workspace(workspace_id, [delta.x, delta.y]);
        }
    }

    fn workspace_visuals(&self, canvas_rect: Rect) -> Vec<WorkspaceVisual> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                let color = Color32::from_rgb(r, g, b);
                let is_active = self.board.active_workspace == Some(workspace.id);
                let (screen_rect, is_empty) = if let Some((min, max)) = self.board.workspace_bounds(workspace.id) {
                    let top_left = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                    let bottom_right = Pos2::new(max[0] + WS_BG_PAD, max[1] + WS_BG_PAD);
                    let screen_min = self.canvas_to_screen(canvas_rect, top_left);
                    let screen_max = self.canvas_to_screen(canvas_rect, bottom_right);
                    (
                        Rect::from_min_max(Pos2::new(screen_min.x, screen_min.y.max(canvas_rect.min.y)), screen_max),
                        false,
                    )
                } else {
                    let screen_min =
                        self.canvas_to_screen(canvas_rect, Pos2::new(workspace.position[0], workspace.position[1]));
                    (
                        Rect::from_min_size(
                            Pos2::new(screen_min.x, screen_min.y.max(canvas_rect.min.y)),
                            Vec2::new(WS_EMPTY_SIZE[0], WS_EMPTY_SIZE[1]),
                        ),
                        true,
                    )
                };

                WorkspaceVisual {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color,
                    screen_rect,
                    label_rect: Rect::from_min_size(
                        screen_rect.min + Vec2::new(14.0, 12.0),
                        Vec2::new(workspace_label_width(&workspace.name), WS_LABEL_HEIGHT),
                    ),
                    is_active,
                    is_empty,
                }
            })
            .collect()
    }
}

impl eframe::App for OrbitermApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }

        self.handle_fullscreen_toggle(ctx);
        self.handle_shortcuts(ctx);
        self.board.process_output();

        for panel_id in self.panels_to_close.drain(..) {
            self.board.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
        }

        for panel_id in self.workspace_creates.drain(..) {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let ws_id = self.board.create_workspace(&name);
            self.board.assign_panel_to_workspace(panel_id, ws_id);
        }
        for (panel_id, ws_id) in self.workspace_assignments.drain(..) {
            self.board.assign_panel_to_workspace(panel_id, ws_id);
        }

        if self.fullscreen_panel.is_some() {
            self.render_fullscreen_panel(ctx);
        } else {
            self.handle_canvas_pan(ctx);
            self.render_toolbar(ctx);
            self.render_canvas(ctx);
            self.render_workspace_backgrounds(ctx);
            self.handle_canvas_double_click(ctx);
            self.render_panels(ctx);
            self.render_canvas_hud(ctx);
        }

        ctx.request_repaint();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

fn viewport_local_rect(ctx: &Context) -> Rect {
    ctx.input(|input| input.viewport().inner_rect.or(input.viewport().outer_rect))
        .map_or_else(
            || {
                let rect = ctx.content_rect();
                Rect::from_min_size(Pos2::ZERO, rect.size())
            },
            |rect| Rect::from_min_size(Pos2::ZERO, rect.size()),
        )
}

#[allow(clippy::too_many_arguments)]
fn paint_panel_chrome(
    ui: &mut egui::Ui,
    panel_rect: Rect,
    titlebar_rect: Rect,
    close_rect: Rect,
    resize_rect: Rect,
    title: &str,
    focused: bool,
    close_hovered: bool,
    ws_accent: Option<Color32>,
) {
    let painter = ui.painter_at(panel_rect);
    let accent = ws_accent.unwrap_or(if focused { theme::ACCENT } else { theme::BORDER_STRONG });

    painter.rect_filled(panel_rect, CornerRadius::same(14), theme::PANEL_BG);
    painter.rect_stroke(
        panel_rect,
        CornerRadius::same(14),
        Stroke::new(1.2, theme::panel_border(accent, focused)),
        StrokeKind::Outside,
    );
    painter.rect_filled(
        titlebar_rect,
        CornerRadius::same(14),
        theme::blend(theme::PANEL_BG_ALT, accent, if focused { 0.18 } else { 0.10 }),
    );
    let title_x = if let Some(ws_color) = ws_accent {
        painter.circle_filled(
            Pos2::new(titlebar_rect.min.x + 14.0, titlebar_rect.center().y),
            4.5,
            ws_color,
        );
        titlebar_rect.min.x + 26.0
    } else {
        titlebar_rect.min.x + 12.0
    };
    painter.text(
        Pos2::new(title_x, titlebar_rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        theme::FG,
    );

    painter.circle_filled(
        close_rect.center(),
        5.0,
        if close_hovered {
            theme::BTN_CLOSE
        } else {
            theme::alpha(theme::FG_DIM, 140)
        },
    );

    let handle_stroke = Stroke::new(1.0, theme::alpha(theme::FG_DIM, 170));
    painter.line_segment(
        [
            resize_rect.right_bottom(),
            resize_rect.left_top() + Vec2::new(6.0, 12.0),
        ],
        handle_stroke,
    );
    painter.line_segment(
        [
            resize_rect.right_bottom() - Vec2::new(0.0, 6.0),
            resize_rect.left_top() + Vec2::new(12.0, 12.0),
        ],
        handle_stroke,
    );
}

fn paint_empty_state(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let card_rect = Rect::from_center_size(rect.center(), Vec2::new(320.0, 110.0));
    let painter = ui.painter();

    painter.rect_filled(card_rect, CornerRadius::same(18), theme::alpha(theme::PANEL_BG, 236));
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(18),
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)),
        StrokeKind::Outside,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 34.0),
        egui::Align2::CENTER_CENTER,
        "Infinite terminal canvas",
        egui::FontId::proportional(17.0),
        theme::FG,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 66.0),
        egui::Align2::CENTER_CENTER,
        "Create a terminal, drag it anywhere, and pan for more space.",
        egui::FontId::proportional(11.5),
        theme::FG_SOFT,
    );
    painter.text(
        Pos2::new(card_rect.center().x, card_rect.min.y + 86.0),
        egui::Align2::CENTER_CENTER,
        "Ctrl+N adds a panel. Middle-drag pans the board.",
        egui::FontId::proportional(10.5),
        theme::FG_DIM,
    );
}

fn render_workspace_visual(
    ctx: &Context,
    workspace: &WorkspaceVisual,
    rename_buffer: Option<&mut String>,
) -> WorkspaceInteraction {
    egui::Area::new(Id::new(("workspace_bg", workspace.id.0)))
        .fixed_pos(workspace.screen_rect.min)
        .constrain(false)
        .order(Order::Background)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(workspace.screen_rect.size(), Sense::hover());
            paint_workspace_frame(ui, rect, workspace.color, workspace.is_active, workspace.is_empty);

            if workspace.is_empty {
                paint_empty_workspace_hint(ui, rect, workspace.label_rect, workspace.color);
            }
        });

    egui::Area::new(Id::new(("workspace_label", workspace.id.0)))
        .fixed_pos(workspace.label_rect.min)
        .constrain(false)
        .order(Order::Tooltip)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(workspace.label_rect.size(), Sense::hover());
            let label_response = ui.interact(
                rect,
                ui.make_persistent_id(("workspace_drag", workspace.id.0)),
                Sense::click_and_drag(),
            );

            if let Some(buffer) = rename_buffer {
                paint_workspace_label_bg(ui, rect, workspace.color, true, false, false);

                let text_rect = Rect::from_min_max(
                    Pos2::new(rect.min.x + 12.0, rect.min.y + 2.0),
                    Pos2::new(rect.max.x - 8.0, rect.max.y - 2.0),
                );
                let mut ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(text_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                let edit = egui::TextEdit::singleline(buffer)
                    .font(egui::FontId::proportional(12.5))
                    .text_color(theme::FG)
                    .frame(false)
                    .desired_width(text_rect.width())
                    .margin(Margin::ZERO);
                let response = ui.add(edit);
                if !response.has_focus() {
                    response.request_focus();
                }

                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
                let lost_focus = response.lost_focus();

                WorkspaceInteraction {
                    activate_workspace: false,
                    drag_delta: Vec2::ZERO,
                    start_rename: false,
                    finish_rename: enter || escape || lost_focus,
                }
            } else {
                if label_response.hovered() || label_response.dragged() {
                    ui.ctx().set_cursor_icon(if label_response.dragged() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    });
                }

                paint_workspace_label(
                    ui,
                    rect,
                    &workspace.name,
                    workspace.color,
                    workspace.is_active,
                    label_response.hovered(),
                    label_response.dragged(),
                );

                WorkspaceInteraction {
                    activate_workspace: label_response.clicked() || label_response.drag_started(),
                    drag_delta: if label_response.dragged() {
                        ctx.input(|input| input.pointer.delta())
                    } else {
                        Vec2::ZERO
                    },
                    start_rename: label_response.double_clicked(),
                    finish_rename: false,
                }
            }
        })
        .inner
}

fn paint_workspace_frame(ui: &mut egui::Ui, rect: Rect, color: Color32, is_active: bool, _is_empty: bool) {
    let painter = ui.painter_at(rect);
    let corner_radius = CornerRadius::same(16);
    let border_alpha = if is_active { 110 } else { 55 };
    let fill_alpha = if is_active { 24 } else { 14 };
    let frame_fill = theme::alpha(theme::blend(theme::PANEL_BG, color, 0.12), fill_alpha);

    painter.rect_filled(rect, corner_radius, frame_fill);
    painter.rect_stroke(
        rect,
        corner_radius,
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

fn paint_workspace_label_bg(
    ui: &mut egui::Ui,
    rect: Rect,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let painter = ui.painter();
    let tint = if dragging {
        0.22
    } else if hovered {
        0.18
    } else if is_active {
        0.14
    } else {
        0.08
    };
    let fill = theme::blend(theme::PANEL_BG_ALT, color, tint);
    let border_alpha = if is_active || hovered { 160 } else { 90 };

    painter.rect_filled(rect, CornerRadius::same(8), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

fn paint_workspace_label(
    ui: &mut egui::Ui,
    rect: Rect,
    name: &str,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    paint_workspace_label_bg(ui, rect, color, is_active, hovered, dragging);

    let painter = ui.painter();
    let grip_center = Pos2::new(rect.max.x - 14.0, rect.center().y);

    painter.circle_filled(
        Pos2::new(rect.min.x + 14.0, rect.center().y),
        4.0,
        theme::alpha(color, if is_active { 220 } else { 150 }),
    );

    painter.text(
        Pos2::new(rect.min.x + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        if is_active { theme::FG } else { theme::FG_SOFT },
    );

    paint_workspace_grip(painter, grip_center, dragging || hovered);
}

fn paint_workspace_grip(painter: &egui::Painter, center: Pos2, highlighted: bool) {
    let color = if highlighted {
        theme::alpha(theme::FG_SOFT, 180)
    } else {
        theme::alpha(theme::FG_DIM, 140)
    };
    let x_offsets = [-3.0, 3.0];
    let y_offsets = [-4.0, 0.0, 4.0];

    for x_offset in x_offsets {
        for y_offset in y_offsets {
            painter.circle_filled(Pos2::new(center.x + x_offset, center.y + y_offset), 1.2, color);
        }
    }
}

fn paint_empty_workspace_hint(ui: &mut egui::Ui, rect: Rect, label_rect: Rect, color: Color32) {
    let painter = ui.painter();
    let copy_pos = Pos2::new(rect.min.x + 18.0, label_rect.max.y + 22.0);

    painter.text(
        copy_pos,
        egui::Align2::LEFT_TOP,
        "Drag this workspace anywhere on the board.",
        egui::FontId::proportional(12.0),
        theme::alpha(theme::FG_SOFT, 210),
    );
    painter.text(
        copy_pos + Vec2::new(0.0, 20.0),
        egui::Align2::LEFT_TOP,
        "New terminals will land inside this frame.",
        egui::FontId::proportional(10.5),
        theme::alpha(theme::blend(theme::FG_DIM, color, 0.18), 196),
    );
}

fn draw_dot_grid(ui: &mut egui::Ui, pan_offset: Vec2) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let spacing = 22.0;
    let dot_radius = 1.15;
    let offset_x = pan_offset.x.rem_euclid(spacing);
    let offset_y = pan_offset.y.rem_euclid(spacing);

    let mut x = rect.min.x + offset_x;
    while x <= rect.max.x {
        let mut y = rect.min.y + offset_y;
        while y <= rect.max.y {
            painter.circle_filled(Pos2::new(x, y), dot_radius, theme::GRID_DOT);
            y += spacing;
        }
        x += spacing;
    }
}

fn paint_canvas_glow(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();

    painter.circle_filled(
        Pos2::new(rect.max.x + 48.0, rect.center().y),
        rect.height() * 0.44,
        theme::CANVAS_WARM_GLOW,
    );
    painter.circle_filled(
        Pos2::new(rect.min.x - 72.0, rect.min.y + rect.height() * 0.16),
        rect.height() * 0.28,
        theme::CANVAS_COOL_GLOW,
    );
}

#[cfg(test)]
fn default_panel_canvas_pos(index: usize) -> Pos2 {
    let column = usize_to_f32(index % 3);
    let row = usize_to_f32(index / 3);
    Pos2::new(120.0 + column * PANEL_COLUMN_SPACING, 120.0 + row * PANEL_ROW_SPACING)
}

fn clamp_panel_size(size: Vec2) -> Vec2 {
    Vec2::new(size.x.max(PANEL_MIN_SIZE[0]), size.y.max(PANEL_MIN_SIZE[1]))
}

fn workspace_label_width(name: &str) -> f32 {
    let estimated_text_width: f32 = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_uppercase() {
                8.6
            } else if ch.is_ascii_whitespace() {
                4.5
            } else {
                7.6
            }
        })
        .sum();

    (estimated_text_width + 60.0).clamp(WS_LABEL_MIN_WIDTH, WS_LABEL_MAX_WIDTH)
}

fn format_grid_position(position: Pos2) -> String {
    format!("{}, {}", rounded_i32(position.x), rounded_i32(position.y))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn rounded_i32(value: f32) -> i32 {
    let rounded = value.round();
    if rounded.is_nan() {
        0
    } else {
        rounded.clamp(i32::MIN as f32, i32::MAX as f32) as i32
    }
}

fn primary_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.5).color(theme::FG))
        .fill(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28))
        .stroke(Stroke::new(
            1.0,
            theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.72),
        ))
        .corner_radius(10)
}

fn chrome_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.0).color(theme::FG_SOFT))
        .fill(theme::PANEL_BG_ALT)
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
        .corner_radius(10)
}

#[cfg(test)]
fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::{PANEL_MIN_SIZE, clamp_panel_size, default_panel_canvas_pos, format_grid_position};
    use egui::{Pos2, Vec2};

    #[test]
    fn default_panel_positions_tile_in_rows() {
        assert_eq!(default_panel_canvas_pos(0), Pos2::new(120.0, 120.0));
        assert_eq!(default_panel_canvas_pos(1), Pos2::new(660.0, 120.0));
        assert_eq!(default_panel_canvas_pos(3), Pos2::new(120.0, 480.0));
    }

    #[test]
    fn panel_size_is_clamped_to_minimums() {
        let clamped = clamp_panel_size(Vec2::new(100.0, 120.0));

        assert!((clamped.x - PANEL_MIN_SIZE[0]).abs() <= f32::EPSILON);
        assert!(clamped.y >= PANEL_MIN_SIZE[1]);
    }

    #[test]
    fn grid_positions_are_rounded_for_display() {
        assert_eq!(format_grid_position(Pos2::new(12.4, -7.6)), "12, -8");
        assert_eq!(format_grid_position(Pos2::new(-3.5, 2.5)), "-4, 3");
    }
}
