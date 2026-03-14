use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use egui::{
    Align, Button, Color32, Context, Id, Key, LayerId, Layout, Margin, Order, Pos2, Rect, Rounding, Sense, Shadow,
    Stroke, Vec2, epaint::CubicBezierShape,
};
use orbiterm_core::{Board, Config, PanelId, PanelKind, PanelOptions, PanelResume, Workspace, WorkspaceId};

use crate::terminal_widget::TerminalView;
use crate::theme;

const DEFAULT_PANEL_WIDTH: f32 = 520.0;
const DEFAULT_PANEL_HEIGHT: f32 = 340.0;
const PANEL_COLUMN_SPACING: f32 = 540.0;
const PANEL_ROW_SPACING: f32 = 360.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
const CONTROL_BAR_HEIGHT: f32 = 92.0;
const WORKSPACE_BADGE_WIDTH: f32 = 220.0;
const WORKSPACE_BADGE_HEIGHT: f32 = 52.0;
const WORKSPACE_PREVIEW_ZOOM: f32 = 0.22;
const AUTO_BOARD_RESIZE_SETTLE_DELAY: Duration = Duration::from_millis(160);
type WorkspaceSnapshot = (WorkspaceId, String, (u8, u8, u8), usize, [f32; 2]);

struct WorkspaceRenameState {
    workspace_id: WorkspaceId,
    draft: String,
    should_focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspaceRenderMode {
    Interactive,
    Preview,
    Badge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarControlsPlacement {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarControl {
    Close,
    Minimize,
    Maximize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TitlebarControlsSpec {
    placement: TitlebarControlsPlacement,
    controls: [TitlebarControl; 3],
}

struct PreviewPanelState {
    workspace_id: WorkspaceId,
    panel_id: PanelId,
    title: String,
    accent: Color32,
    screen_rect: Rect,
    focused: bool,
    lines: Vec<String>,
}

#[derive(Clone, Copy)]
struct WorkspaceTitleStyle {
    text_size: f32,
    width_bounds: [f32; 2],
}

#[allow(clippy::struct_excessive_bools)]
pub struct OrbitermApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    new_workspace_name: String,
    theme_applied: bool,
    zoom: f32,
    pan_offset: Vec2,
    panel_canvas_rects: HashMap<PanelId, Rect>,
    panel_screen_rects: HashMap<PanelId, Rect>,
    panel_connection_points: HashMap<PanelId, Pos2>,
    workspace_badge_rects: HashMap<WorkspaceId, Rect>,
    workspace_canvas_rects: HashMap<WorkspaceId, Rect>,
    workspace_rename: Option<WorkspaceRenameState>,
    pending_fit_at: Option<Instant>,
    last_viewport_size: Option<Vec2>,
    config_path: Option<PathBuf>,
    show_config_editor: bool,
    config_text: String,
    config_last_modified: Option<SystemTime>,
}

impl OrbitermApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, config: &Config, config_path: Option<PathBuf>) -> Self {
        let board = Board::from_config(config).unwrap_or_else(|error| {
            tracing::error!("failed to load config: {error}");
            Board::new()
        });

        let config_text = config_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        let config_last_modified = config_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());

        Self {
            board,
            panels_to_close: Vec::new(),
            new_workspace_name: String::new(),
            theme_applied: false,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            panel_canvas_rects: HashMap::new(),
            panel_screen_rects: HashMap::new(),
            panel_connection_points: HashMap::new(),
            workspace_badge_rects: HashMap::new(),
            workspace_canvas_rects: HashMap::new(),
            workspace_rename: None,
            pending_fit_at: Some(Instant::now()),
            last_viewport_size: None,
            config_path,
            show_config_editor: false,
            config_text,
            config_last_modified,
        }
    }

    fn canvas_to_screen(&self, position: Pos2) -> Pos2 {
        Pos2::new(
            position.x * self.zoom + self.pan_offset.x,
            position.y * self.zoom + self.pan_offset.y,
        )
    }

    fn screen_to_canvas(&self, position: Pos2) -> Pos2 {
        Pos2::new(
            (position.x - self.pan_offset.x) / self.zoom,
            (position.y - self.pan_offset.y) / self.zoom,
        )
    }

    fn reset_view(&mut self, _ctx: &Context) {
        self.zoom = 1.0;
        self.pan_offset = Vec2::ZERO;
    }

    fn remove_empty_workspaces(&mut self) {
        let empty_ids: Vec<WorkspaceId> = self
            .board
            .workspaces
            .iter()
            .filter(|ws| ws.panels.is_empty())
            .map(|ws| ws.id)
            .collect();
        for id in empty_ids {
            self.board.remove_workspace(id);
            self.workspace_badge_rects.remove(&id);
            self.workspace_canvas_rects.remove(&id);
        }
    }

    fn reset_layout_cache(&mut self) {
        self.panel_canvas_rects.clear();
        self.panel_screen_rects.clear();
        self.panel_connection_points.clear();
        self.workspace_badge_rects.clear();
        self.workspace_canvas_rects.clear();
        self.workspace_rename = None;
        self.pending_fit_at = Some(Instant::now());
        self.last_viewport_size = None;
    }

    fn start_workspace_rename(&mut self, workspace_id: WorkspaceId, current_name: &str) {
        self.workspace_rename = Some(WorkspaceRenameState {
            workspace_id,
            draft: current_name.to_owned(),
            should_focus: true,
        });
    }

    fn commit_workspace_rename(&mut self) {
        if let Some(rename) = self.workspace_rename.take() {
            let _ = self.board.rename_workspace(rename.workspace_id, rename.draft.trim());
        }
    }

    fn is_renaming_workspace(&self, workspace_id: WorkspaceId) -> bool {
        self.workspace_rename
            .as_ref()
            .is_some_and(|rename| rename.workspace_id == workspace_id)
    }

    fn schedule_fit_board(&mut self, delay: Duration) {
        self.pending_fit_at = Some(Instant::now() + delay);
    }

    fn handle_zoom(&mut self, ctx: &Context) {
        let zoom_delta = ctx.input(egui::InputState::zoom_delta);
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            self.zoom = (self.zoom * zoom_delta).clamp(0.45, 2.5);
            self.pending_fit_at = None;
        }

        if ctx.input(|input| input.key_pressed(Key::Num0) && input.modifiers.ctrl) {
            self.fit_view_to_content(ctx);
        }
    }

    fn handle_canvas_pan(&mut self, ctx: &Context) {
        let canvas_rect = Self::canvas_view_rect(ctx);
        let wants_keyboard_input = ctx.wants_keyboard_input();
        let pan_delta = ctx.input(|input| {
            let pointer_position = input.pointer.hover_pos();
            let pointer_in_canvas = pointer_position
                .zip(canvas_rect)
                .is_some_and(|(position, rect)| rect.contains(position));
            let pointer_over_panel = pointer_position
                .is_some_and(|position| self.panel_screen_rects.values().any(|rect| rect.contains(position)));
            let drag_panning = pointer_in_canvas
                && (input.pointer.middle_down() || (input.modifiers.ctrl && input.pointer.primary_down()));
            let scroll_pan_enabled = pointer_in_canvas && !wants_keyboard_input && !pointer_over_panel;

            if drag_panning {
                input.pointer.delta()
            } else if scroll_pan_enabled && !input.modifiers.ctrl {
                input.smooth_scroll_delta + input.raw_scroll_delta
            } else {
                Vec2::ZERO
            }
        });

        if pan_delta != Vec2::ZERO {
            self.pan_offset += pan_delta;
            self.pending_fit_at = None;
        }
    }

    fn handle_canvas_workspace_creation(&mut self, ctx: &Context) {
        let Some(canvas_rect) = Self::canvas_view_rect(ctx) else {
            return;
        };
        let should_create = ctx
            .input(|input| input.modifiers.ctrl && input.pointer.button_double_clicked(egui::PointerButton::Primary));
        let Some(pointer_position) = ctx.input(|input| input.pointer.latest_pos()) else {
            return;
        };

        let panel_rects: Vec<Rect> = self.panel_screen_rects.values().copied().collect();
        let workspace_rects: Vec<Rect> = self.workspace_badge_rects.values().copied().collect();
        if !should_create_workspace_from_canvas_gesture(
            should_create,
            canvas_rect,
            pointer_position,
            &panel_rects,
            &workspace_rects,
        ) {
            return;
        }

        let name = self.next_workspace_name();
        let canvas_position = self.screen_to_canvas(pointer_position);
        self.create_workspace_at(&name, canvas_position);
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        if self.terminal_accepts_keyboard_input(ctx) {
            return;
        }

        if ctx.input(|input| input.key_pressed(Key::N) && input.modifiers.ctrl && input.modifiers.shift) {
            self.create_panel_in_workspace(None);
        }

        if ctx.input(|input| input.key_pressed(Key::Comma) && input.modifiers.ctrl) {
            self.show_config_editor = !self.show_config_editor;
        }
    }

    fn create_workspace_named(&mut self, ctx: &Context, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }

        let position = self.suggest_workspace_position(ctx);
        self.create_workspace_at(trimmed, position);
    }

    fn next_workspace_name(&self) -> String {
        let mut index = self.board.workspaces.len() + 1;
        loop {
            let candidate = format!("workspace {index}");
            let exists = self
                .board
                .workspaces
                .iter()
                .any(|workspace| workspace.name.eq_ignore_ascii_case(&candidate));
            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    fn create_workspace_at(&mut self, name: &str, position: Pos2) {
        let workspace_id = self.board.create_workspace(name);
        if let Some(workspace) = self.board.workspace_mut(workspace_id) {
            workspace.position = [position.x, position.y];
        }
        if self
            .board
            .create_panel(PanelOptions::default(), Some(workspace_id))
            .is_ok()
        {
            self.schedule_fit_board(Duration::ZERO);
        }
        self.board.focus_workspace(workspace_id);
    }

    fn suggest_workspace_position(&self, ctx: &Context) -> Pos2 {
        if let Some(bounds) = self.layout_bounds() {
            return Pos2::new(bounds.max.x + 120.0, bounds.min.y.max(48.0));
        }

        let center = Self::canvas_view_rect(ctx).map_or(Pos2::new(280.0, 180.0), |rect| rect.center());
        self.screen_to_canvas(center + Vec2::new(-180.0, -90.0))
    }

    fn preferred_workspace(&self, workspace_id: Option<WorkspaceId>) -> Option<WorkspaceId> {
        workspace_id
            .or(self.board.active_workspace)
            .or_else(|| self.board.workspaces.first().map(|workspace| workspace.id))
    }

    fn create_panel_in_workspace(&mut self, workspace_id: Option<WorkspaceId>) {
        self.create_panel_with_options(workspace_id, PanelOptions::default());
    }

    fn create_agent_panel(&mut self, workspace_id: Option<WorkspaceId>, kind: PanelKind) {
        let (name, resume, auto_resize_pty) = agent_panel_defaults(kind);
        self.create_panel_with_options(
            workspace_id,
            PanelOptions {
                name: Some(name.to_string()),
                kind,
                resume,
                auto_resize_pty,
                ..PanelOptions::default()
            },
        );
    }

    fn create_panel_with_options(&mut self, workspace_id: Option<WorkspaceId>, opts: PanelOptions) {
        let target_workspace = self.preferred_workspace(workspace_id);
        if self.board.create_panel(opts, target_workspace).is_ok() {
            self.schedule_fit_board(Duration::ZERO);
        }
        if let Some(workspace_id) = target_workspace {
            self.board.focus_workspace(workspace_id);
        }
    }

    fn render_canvas(&self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ctx, |ui| {
                paint_canvas_glow(ui);
                draw_dot_grid(ui, self.pan_offset, self.zoom);
            });
    }

    fn render_titlebar(&self, ctx: &Context) {
        let controls = native_titlebar_controls_spec();
        egui::TopBottomPanel::top("titlebar")
            .exact_height(TITLEBAR_HEIGHT)
            .frame(
                egui::Frame::default()
                    .fill(theme::TITLEBAR_BG)
                    .inner_margin(Margin::symmetric(14.0, 0.0))
                    .stroke(Stroke::new(
                        1.0,
                        theme::blend(theme::BORDER_SUBTLE, theme::ACCENT, 0.15),
                    )),
            )
            .show(ctx, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if controls.placement == TitlebarControlsPlacement::Leading {
                        ui.add_space(2.0);
                        render_titlebar_window_controls(ui, ctx, controls.controls);
                        ui.add_space(14.0);
                    } else {
                        ui.add_space(4.0);
                    }
                    ui.label(
                        egui::RichText::new(crate::branding::APP_NAME)
                            .color(theme::FG_SOFT)
                            .size(13.0)
                            .strong(),
                    );

                    ui.add_space(5.0);
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(3.0), Sense::hover());
                    ui.painter().circle_filled(dot_rect.center(), 1.5, theme::FG_DIM);
                    ui.add_space(5.0);

                    ui.label(
                        egui::RichText::new(crate::branding::APP_TAGLINE)
                            .color(theme::alpha(theme::FG_DIM, 160))
                            .size(10.5),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(4.0);
                        if controls.placement == TitlebarControlsPlacement::Trailing {
                            render_titlebar_window_controls(ui, ctx, controls.controls);
                            ui.add_space(10.0);
                        }
                        paint_status_pill(ui, &format!("{:.0}%", self.zoom * 100.0));
                        ui.add_space(2.0);
                        paint_status_pill(ui, &format!("{} panels", self.board.panels.len()));
                        ui.add_space(2.0);
                        paint_status_pill(ui, &format!("{} workspaces", self.board.workspaces.len()));

                        let drag_area = ui.available_rect_before_wrap();
                        let response = ui.allocate_rect(drag_area, Sense::click_and_drag());
                        if response.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        if response.double_clicked() {
                            let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                    });
                });
            });
    }

    fn render_toolbar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("toolbar")
            .exact_height(CONTROL_BAR_HEIGHT)
            .frame(
                egui::Frame::default()
                    .fill(theme::TOOLBAR_BG)
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .stroke(Stroke::new(
                        1.0,
                        theme::blend(theme::BORDER_SUBTLE, theme::ACCENT, 0.12),
                    )),
            )
            .show(ctx, |ui| {
                let input_width = (ui.available_width() * 0.22).clamp(128.0, 220.0);

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("workspaces")
                                .color(theme::FG_DIM)
                                .size(11.0)
                                .strong(),
                        );
                        ui.add_space(4.0);

                        let input_response = ui.add(
                            egui::TextEdit::singleline(&mut self.new_workspace_name)
                                .desired_width(input_width)
                                .hint_text("new workspace"),
                        );
                        let create_from_enter =
                            input_response.has_focus() && ui.input(|input| input.key_pressed(Key::Enter));

                        if create_from_enter || ui.add(primary_button("+ Workspace")).clicked() {
                            let fallback_name = self.next_workspace_name();
                            let name = workspace_name_from_input(&self.new_workspace_name, &fallback_name);
                            self.create_workspace_named(ctx, &name);
                            self.new_workspace_name.clear();
                        }

                        if ui.add(chrome_button("+ Shell")).clicked() {
                            self.create_panel_in_workspace(None);
                        }

                        if ui.add(chrome_button("+ Codex")).clicked() {
                            self.create_agent_panel(None, PanelKind::Codex);
                        }

                        if ui.add(chrome_button("+ Claude")).clicked() {
                            self.create_agent_panel(None, PanelKind::Claude);
                        }
                    });

                    ui.add_space(6.0);
                    let line_rect = ui.available_rect_before_wrap();
                    ui.painter().hline(
                        line_rect.x_range(),
                        line_rect.top(),
                        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE, 100)),
                    );
                    ui.add_space(6.0);
                    self.render_workspace_strip(ui);
                });
            });
    }

    fn render_workspace_strip(&mut self, ui: &mut egui::Ui) {
        let workspaces = self.workspace_snapshots();

        if workspaces.is_empty() {
            ui.label(egui::RichText::new("No workspaces yet").color(theme::FG_DIM).size(11.0));
            return;
        }

        egui::ScrollArea::horizontal()
            .id_salt("workspace_strip")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (workspace_id, name, accent, count, _position) in workspaces {
                        self.render_workspace_chip(
                            ui,
                            workspace_id,
                            &name,
                            Color32::from_rgb(accent.0, accent.1, accent.2),
                            count,
                            self.board.active_workspace == Some(workspace_id),
                        );
                    }
                });
            });
    }

    fn render_workspace_chip(
        &mut self,
        ui: &mut egui::Ui,
        workspace_id: WorkspaceId,
        name: &str,
        accent: Color32,
        count: usize,
        active: bool,
    ) {
        let editing = self.is_renaming_workspace(workspace_id);
        let mut add_terminal = false;

        egui::Frame::default()
            .fill(theme::workspace_fill(accent))
            .rounding(Rounding::same(15.0))
            .inner_margin(Margin::symmetric(10.0, 6.0))
            .stroke(Stroke::new(1.0, theme::workspace_border(accent, editing || active)))
            .shadow(theme::workspace_shadow(accent))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    paint_workspace_dot(ui, accent, 4.5);
                    let _ = self.render_workspace_name_control(
                        ui,
                        workspace_id,
                        name,
                        accent,
                        WorkspaceTitleStyle {
                            text_size: 12.0,
                            width_bounds: [78.0, 180.0],
                        },
                        true,
                    );
                    render_count_badge(ui, accent, count);

                    if ui.add(icon_button("+")).clicked() {
                        add_terminal = true;
                    }
                });
            });

        if add_terminal {
            self.create_panel_in_workspace(Some(workspace_id));
        }
    }

    fn render_workspace_name_control(
        &mut self,
        ui: &mut egui::Ui,
        workspace_id: WorkspaceId,
        current_name: &str,
        accent: Color32,
        style: WorkspaceTitleStyle,
        interactive: bool,
    ) -> Rect {
        if self.is_renaming_workspace(workspace_id) {
            let mut commit = false;
            let mut cancel = false;
            let mut response_rect = Rect::NOTHING;

            if let Some(rename) = self.workspace_rename.as_mut() {
                let [min_width, max_width] = style.width_bounds;
                let desired_width =
                    (usize_to_f32(rename.draft.chars().count().max(8)) * 8.0).clamp(min_width, max_width);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut rename.draft)
                        .id(ui.make_persistent_id(("workspace_rename", workspace_id.0)))
                        .desired_width(desired_width)
                        .font(egui::TextStyle::Button),
                );
                response_rect = response.rect;

                if rename.should_focus {
                    response.request_focus();
                    rename.should_focus = false;
                }

                commit = ui.input(|input| input.key_pressed(Key::Enter))
                    || (response.lost_focus() && ui.input(|input| input.pointer.any_released()));
                cancel = ui.input(|input| input.key_pressed(Key::Escape));
            }

            if cancel {
                self.workspace_rename = None;
            } else if commit {
                self.commit_workspace_rename();
            }

            response_rect
        } else {
            let title_rect = paint_workspace_title_pill(ui, current_name, accent, style.text_size, style.width_bounds);
            if interactive {
                let response = ui.interact(
                    title_rect,
                    ui.make_persistent_id(("workspace_name", workspace_id.0)),
                    Sense::click(),
                );
                if response.clicked() {
                    self.board.focus_workspace(workspace_id);
                }
                if response.double_clicked() {
                    self.start_workspace_rename(workspace_id, current_name);
                }
            }

            title_rect
        }
    }

    fn render_workspace_badges(&mut self, ctx: &Context) {
        self.workspace_badge_rects.clear();
        self.workspace_canvas_rects.clear();
        let constrain_rect = Self::canvas_view_rect(ctx).unwrap_or_else(|| viewport_local_rect(ctx));

        let workspaces = self.workspace_snapshots();

        for (workspace_id, name, accent, count, position) in workspaces {
            let accent = Color32::from_rgb(accent.0, accent.1, accent.2);
            let current_pos = self.canvas_to_screen(Pos2::new(position[0], position[1]));
            let desired_rect = clamp_rect_to_bounds(
                Rect::from_min_size(current_pos, Vec2::new(WORKSPACE_BADGE_WIDTH, WORKSPACE_BADGE_HEIGHT)),
                constrain_rect,
            );
            let editing = self.is_renaming_workspace(workspace_id);
            let active = self.board.active_workspace == Some(workspace_id);
            let mut add_terminal = false;
            let mut title_rect = Rect::NOTHING;

            let area = egui::Area::new(Id::new(("workspace_badge", workspace_id.0)))
                .current_pos(desired_rect.min)
                .constrain_to(constrain_rect)
                .movable(!editing)
                .sense(Sense::click_and_drag())
                .order(Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::default()
                        .fill(theme::workspace_fill(accent))
                        .rounding(Rounding::same(18.0))
                        .inner_margin(Margin::symmetric(14.0, 9.0))
                        .stroke(Stroke::new(1.2, theme::workspace_border(accent, editing || active)))
                        .shadow(theme::workspace_shadow(accent))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                paint_workspace_dot(ui, accent, 5.5);
                                title_rect = self.render_workspace_name_control(
                                    ui,
                                    workspace_id,
                                    &name,
                                    accent,
                                    WorkspaceTitleStyle {
                                        text_size: 13.5,
                                        width_bounds: [86.0, 220.0],
                                    },
                                    false,
                                );
                                render_count_badge(ui, accent, count);

                                if ui.add(icon_button("+")).clicked() {
                                    add_terminal = true;
                                }
                            });
                        });
                });

            let constrained_rect = clamp_rect_to_bounds(area.response.rect, constrain_rect);
            self.workspace_badge_rects.insert(workspace_id, constrained_rect);

            let canvas_position = self.screen_to_canvas(constrained_rect.min);
            let previous_canvas_position = Pos2::new(position[0], position[1]);
            let drag_delta = canvas_position - previous_canvas_position;
            self.workspace_canvas_rects.insert(
                workspace_id,
                Rect::from_min_size(canvas_position, constrained_rect.size() / self.zoom),
            );
            if area.response.clicked() || area.response.drag_started() {
                self.board.focus_workspace(workspace_id);
            }
            let title_was_double_clicked = area.response.double_clicked()
                && ctx
                    .input(|input| input.pointer.interact_pos())
                    .is_some_and(|pointer_position| title_rect.contains(pointer_position));
            if title_was_double_clicked {
                self.start_workspace_rename(workspace_id, &name);
            } else if area.response.double_clicked() {
                self.fit_view_to_workspace(ctx, workspace_id);
            }
            if self
                .board
                .move_workspace(workspace_id, [canvas_position.x, canvas_position.y])
                && drag_delta != Vec2::ZERO
            {
                self.pending_fit_at = None;
            }

            if add_terminal {
                self.create_panel_in_workspace(Some(workspace_id));
            }
        }
    }

    fn render_island_backgrounds(&mut self, ctx: &Context) {
        let painter = ctx.layer_painter(LayerId::background());

        for workspace in &self.board.workspaces {
            if workspace.panels.is_empty() {
                continue;
            }

            let accent_rgb = workspace.accent();
            let accent = Color32::from_rgb(accent_rgb.0, accent_rgb.1, accent_rgb.2);
            let canvas_rect = self.workspace_canvas_bounds(workspace);
            let bg_rect = Rect::from_min_max(
                self.canvas_to_screen(canvas_rect.min),
                self.canvas_to_screen(canvas_rect.max),
            );

            let fill = theme::alpha(accent, 14);
            painter.rect_filled(bg_rect, Rounding::same(16.0 * self.zoom), fill);

            let border_stroke = Stroke::new(1.0, theme::alpha(accent, 40));
            painter.rect_stroke(bg_rect, Rounding::same(16.0 * self.zoom), border_stroke);
        }
    }

    fn draw_connectors(&self, ctx: &Context) {
        let painter = ctx.layer_painter(LayerId::background());

        for workspace in &self.board.workspaces {
            let Some(badge_rect) = self.workspace_badge_rects.get(&workspace.id) else {
                continue;
            };

            let accent = workspace.accent();
            let accent = Color32::from_rgb(accent.0, accent.1, accent.2);
            let start = Pos2::new(badge_rect.right() - 20.0, badge_rect.center().y);

            for panel_id in &workspace.panels {
                let Some(&end) = self.panel_connection_points.get(panel_id) else {
                    continue;
                };

                let bend = ((end.x - start.x).abs() * 0.35).max(70.0);
                let curve = CubicBezierShape::from_points_stroke(
                    [start, start + Vec2::new(bend, 0.0), end - Vec2::new(bend, 0.0), end],
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(1.1, theme::alpha(accent, 118)),
                );

                painter.add(curve);
                painter.circle_filled(start, 2.5, theme::alpha(accent, 160));
                painter.circle_filled(end, 3.0, theme::alpha(accent, 190));
            }
        }
    }

    fn render_preview_panels(&mut self, ctx: &Context) {
        let constrain_rect = Self::canvas_view_rect(ctx).unwrap_or_else(|| viewport_local_rect(ctx));
        for preview in self.preview_panel_states() {
            let constrained_rect = clamp_rect_to_bounds(preview.screen_rect, constrain_rect);
            let constrained_canvas_rect = Rect::from_min_size(
                self.screen_to_canvas(constrained_rect.min),
                constrained_rect.size() / self.zoom,
            );
            self.panel_canvas_rects
                .insert(preview.panel_id, constrained_canvas_rect);
            self.panel_connection_points.insert(
                preview.panel_id,
                Pos2::new(
                    constrained_rect.center().x,
                    constrained_rect.min.y + constrained_rect.height() * 0.10,
                ),
            );

            let response = egui::Area::new(Id::new(("panel_preview", preview.panel_id.0)))
                .current_pos(constrained_rect.min)
                .constrain_to(constrain_rect)
                .order(Order::Middle)
                .show(ctx, |ui| {
                    let (rect, response) = ui.allocate_exact_size(constrained_rect.size(), Sense::click());
                    paint_panel_preview(
                        ui,
                        rect,
                        &preview.title,
                        &preview.lines,
                        preview.accent,
                        preview.focused,
                        self.zoom,
                    );
                    response
                });

            if response.inner.clicked() {
                self.board.focus(preview.panel_id);
                self.board.focus_workspace(preview.workspace_id);
            }
            if response.inner.double_clicked() {
                self.fit_view_to_workspace(ctx, preview.workspace_id);
            }
        }
    }

    fn render_live_panels(&mut self, ctx: &Context) {
        let constrain_rect = Self::canvas_view_rect(ctx).unwrap_or_else(|| viewport_local_rect(ctx));
        let interactive_workspace = self.board.active_workspace.and_then(|workspace_id| {
            (self.workspace_render_mode(workspace_id) == WorkspaceRenderMode::Interactive).then_some(workspace_id)
        });
        let panel_info: Vec<_> = self
            .board
            .panels
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.workspace_id.is_none() || panel.workspace_id == interactive_workspace)
            .map(|(index, panel)| {
                let accent = panel
                    .workspace_id
                    .and_then(|workspace_id| self.board.workspace(workspace_id))
                    .map(orbiterm_core::Workspace::accent);

                (panel.id, panel.title.clone(), accent, index)
            })
            .collect();

        let mut panels_to_close = Vec::new();

        for (panel_id, title, accent, index) in panel_info {
            let accent = accent.map_or(theme::BORDER_STRONG, |color| {
                Color32::from_rgb(color.0, color.1, color.2)
            });
            if self.render_live_panel(ctx, constrain_rect, panel_id, &title, accent, index) {
                panels_to_close.push(panel_id);
            }
        }

        self.panels_to_close = panels_to_close;
    }

    fn render_live_panel(
        &mut self,
        ctx: &Context,
        constrain_rect: Rect,
        panel_id: PanelId,
        title: &str,
        accent: Color32,
        fallback_index: usize,
    ) -> bool {
        let current_canvas_position = self
            .panel_canvas_position(panel_id)
            .unwrap_or_else(|| Self::default_panel_canvas_pos(fallback_index));
        let current_screen_position = self.canvas_to_screen(current_canvas_position);
        let is_focused = self.board.focused == Some(panel_id);
        let mut open = true;
        let canvas_size = self
            .board
            .panel(panel_id)
            .map_or(Vec2::new(DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT), |panel| {
                Vec2::new(panel.layout.size[0], panel.layout.size[1])
            });
        let screen_size = canvas_size * self.zoom;

        let response = egui::Window::new(title)
            .id(Id::new(("panel", panel_id.0)))
            .open(&mut open)
            .current_pos(current_screen_position)
            .default_size(screen_size)
            .min_size(Vec2::new(DEFAULT_PANEL_WIDTH * 0.4, DEFAULT_PANEL_HEIGHT * 0.4))
            .constrain_to(constrain_rect)
            .collapsible(false)
            .resizable(true)
            .frame(
                egui::Frame::default()
                    .fill(theme::PANEL_BG)
                    .rounding(Rounding::same(14.0))
                    .inner_margin(6.0)
                    .stroke(Stroke::new(1.1, theme::panel_border(accent, is_focused)))
                    .shadow(theme::panel_shadow(accent, is_focused)),
            )
            .show(ctx, |ui| {
                if let Some(panel) = self.board.panels.iter_mut().find(|item| item.id == panel_id) {
                    let pty_resized = render_pty_controls(ui, panel);
                    ui.add_space(6.0);
                    let clicked_terminal = TerminalView::new(panel).show(ui, self.board.focused == Some(panel_id));
                    (clicked_terminal, pty_resized)
                } else {
                    (false, false)
                }
            });

        if let Some(window) = response {
            self.sync_live_panel_geometry(panel_id, window.response.rect);

            let (clicked_terminal, _) = window.inner.unwrap_or((false, false));
            if clicked_terminal || window.response.clicked() || window.response.drag_started() {
                self.board.focus(panel_id);
            }
        }

        !open
    }

    fn sync_live_panel_geometry(&mut self, panel_id: PanelId, window_rect: Rect) {
        self.panel_screen_rects.insert(panel_id, window_rect);
        self.panel_connection_points
            .insert(panel_id, Pos2::new(window_rect.center().x, window_rect.min.y + 14.0));

        let canvas_position = self.screen_to_canvas(window_rect.min);
        let canvas_size = Vec2::new(window_rect.width() / self.zoom, window_rect.height() / self.zoom);
        self.panel_canvas_rects
            .insert(panel_id, Rect::from_min_size(canvas_position, canvas_size));

        if let Some(workspace_id) = self.board.panel_workspace_id(panel_id)
            && let Some(workspace) = self.board.workspace(workspace_id)
        {
            let relative_position = canvas_position - Pos2::new(workspace.position[0], workspace.position[1]);
            let _ = self
                .board
                .move_panel(panel_id, [relative_position.x, relative_position.y]);
        } else {
            let _ = self.board.move_panel(panel_id, [canvas_position.x, canvas_position.y]);
        }
        let _ = self.board.resize_panel(panel_id, [canvas_size.x, canvas_size.y]);
    }

    fn preview_panel_states(&self) -> Vec<PreviewPanelState> {
        let mut previews = Vec::new();

        for workspace in &self.board.workspaces {
            if self.workspace_render_mode(workspace.id) != WorkspaceRenderMode::Preview {
                continue;
            }

            let accent = workspace.accent();
            let accent = Color32::from_rgb(accent.0, accent.1, accent.2);
            for panel_id in &workspace.panels {
                let Some(panel) = self.board.panel(*panel_id) else {
                    continue;
                };

                let canvas_rect = self.panel_canvas_rect(panel);
                let screen_rect =
                    Rect::from_min_size(self.canvas_to_screen(canvas_rect.min), canvas_rect.size() * self.zoom);
                let line_count = if screen_rect.height() < 140.0 {
                    3
                } else if screen_rect.height() < 220.0 {
                    5
                } else {
                    7
                };
                let char_budget = preview_char_budget(screen_rect.width());

                previews.push(PreviewPanelState {
                    workspace_id: workspace.id,
                    panel_id: panel.id,
                    title: panel.title.clone(),
                    accent,
                    screen_rect,
                    focused: self.board.focused == Some(panel.id),
                    lines: preview_lines(panel, line_count, char_budget),
                });
            }
        }

        previews
    }
    fn default_panel_canvas_pos(fallback_index: usize) -> Pos2 {
        let column = fallback_index % 3;
        let row = fallback_index / 3;
        Pos2::new(
            140.0 + usize_to_f32(column) * PANEL_COLUMN_SPACING,
            140.0 + usize_to_f32(row) * PANEL_ROW_SPACING,
        )
    }

    fn panel_canvas_position(&self, panel_id: PanelId) -> Option<Pos2> {
        let panel = self.board.panel(panel_id)?;
        let local_position = Pos2::new(panel.layout.position[0], panel.layout.position[1]);
        if let Some(workspace_id) = panel.workspace_id
            && let Some(workspace) = self.board.workspace(workspace_id)
        {
            return Some(Pos2::new(workspace.position[0], workspace.position[1]) + local_position.to_vec2());
        }

        Some(local_position)
    }

    fn check_config_reload(&mut self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let Ok(metadata) = std::fs::metadata(path) else {
            return;
        };
        let Ok(modified) = metadata.modified() else {
            return;
        };

        if self
            .config_last_modified
            .is_some_and(|last_modified| modified > last_modified)
        {
            tracing::info!("config changed on disk, reloading");
            self.config_last_modified = Some(modified);

            if let Ok(text) = std::fs::read_to_string(path) {
                self.config_text.clone_from(&text);
                if let Ok(config) = serde_yaml::from_str::<orbiterm_core::Config>(&text)
                    && let Ok(board) = Board::from_config(&config)
                {
                    self.board = board;
                    self.reset_layout_cache();
                    tracing::info!("config reloaded successfully");
                }
            }
        }
    }

    fn render_config_editor(&mut self, ctx: &Context) {
        egui::Window::new("Config Editor")
            .id(Id::new("config_editor"))
            .default_size([560.0, 420.0])
            .resizable(true)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED)
                    .rounding(Rounding::same(12.0))
                    .inner_margin(10.0)
                    .stroke(Stroke::new(
                        1.0,
                        theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.55),
                    ))
                    .shadow(Shadow {
                        offset: [0.0, 8.0].into(),
                        blur: 26.0,
                        spread: 2.0,
                        color: Color32::from_black_alpha(110),
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Ctrl+, toggles this editor")
                            .color(theme::FG_DIM)
                            .size(10.0),
                    );

                    if let Some(path) = &self.config_path {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .color(theme::FG_DIM)
                                .size(10.0),
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary_button("Save & Apply")).clicked() {
                            self.save_and_apply_config();
                        }
                    });
                });

                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.config_text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(22),
                    );
                });
            });
    }

    fn save_and_apply_config(&mut self) {
        match serde_yaml::from_str::<orbiterm_core::Config>(&self.config_text) {
            Ok(config) => {
                if let Some(path) = &self.config_path {
                    if let Err(error) = std::fs::write(path, &self.config_text) {
                        tracing::error!("failed to write config: {error}");
                        return;
                    }

                    self.config_last_modified = std::fs::metadata(path)
                        .ok()
                        .and_then(|metadata| metadata.modified().ok());
                }

                match Board::from_config(&config) {
                    Ok(board) => {
                        self.board = board;
                        self.reset_layout_cache();
                        tracing::info!("config applied");
                    }
                    Err(error) => tracing::error!("failed to apply config: {error}"),
                }
            }
            Err(error) => tracing::error!("invalid YAML: {error}"),
        }
    }

    fn workspace_snapshots(&self) -> Vec<WorkspaceSnapshot> {
        let mut workspaces: Vec<_> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                (
                    workspace.id,
                    workspace.name.clone(),
                    workspace.accent(),
                    workspace.panels.len(),
                    workspace.position,
                )
            })
            .collect();

        workspaces.sort_by(|left, right| {
            left.4[1]
                .total_cmp(&right.4[1])
                .then_with(|| left.4[0].total_cmp(&right.4[0]))
        });
        workspaces
    }

    fn workspace_render_mode(&self, workspace_id: WorkspaceId) -> WorkspaceRenderMode {
        if self.board.active_workspace == Some(workspace_id) {
            WorkspaceRenderMode::Interactive
        } else if self.zoom >= WORKSPACE_PREVIEW_ZOOM {
            WorkspaceRenderMode::Preview
        } else {
            WorkspaceRenderMode::Badge
        }
    }

    fn workspace_canvas_bounds(&self, workspace: &Workspace) -> Rect {
        let mut bounds = Rect::from_min_size(
            Pos2::new(workspace.position[0], workspace.position[1]),
            Vec2::new(WORKSPACE_BADGE_WIDTH, WORKSPACE_BADGE_HEIGHT),
        );

        for panel_id in &workspace.panels {
            let Some(panel) = self.board.panel(*panel_id) else {
                continue;
            };
            bounds = bounds.union(self.panel_canvas_rect(panel));
        }

        bounds.expand2(Vec2::new(36.0, 36.0))
    }

    fn panel_canvas_rect(&self, panel: &orbiterm_core::Panel) -> Rect {
        let position = self
            .panel_canvas_position(panel.id)
            .unwrap_or_else(|| Pos2::new(panel.layout.position[0], panel.layout.position[1]));
        Rect::from_min_size(position, Vec2::new(panel.layout.size[0], panel.layout.size[1]))
    }

    fn terminal_accepts_keyboard_input(&self, ctx: &Context) -> bool {
        self.board.focused.is_some() && !ctx.wants_keyboard_input()
    }

    fn canvas_view_rect(ctx: &Context) -> Option<Rect> {
        let rect = viewport_local_rect(ctx);
        (rect.width() > 0.0 && rect.height() > 0.0).then(|| {
            Rect::from_min_max(
                Pos2::new(rect.min.x, rect.min.y + TITLEBAR_HEIGHT),
                Pos2::new(rect.max.x, rect.max.y - CONTROL_BAR_HEIGHT),
            )
        })
    }

    fn fit_view_to_content(&mut self, ctx: &Context) {
        self.fit_bounds(ctx, self.fit_board_bounds(), 0.45);
    }

    fn fit_view_to_workspace(&mut self, ctx: &Context, workspace_id: WorkspaceId) {
        self.board.focus_workspace(workspace_id);
        let bounds = self
            .board
            .workspace(workspace_id)
            .map(|workspace| self.workspace_canvas_bounds(workspace));
        self.fit_bounds(ctx, bounds, 0.45);
    }

    fn fit_bounds(&mut self, ctx: &Context, bounds: Option<Rect>, min_zoom: f32) {
        let Some(content_bounds) = bounds else {
            self.reset_view(ctx);
            self.pending_fit_at = None;
            return;
        };
        let Some(canvas_rect) = Self::canvas_view_rect(ctx) else {
            self.schedule_fit_board(AUTO_BOARD_RESIZE_SETTLE_DELAY);
            return;
        };

        let margin = Vec2::new(72.0, 56.0);
        let available_size = Vec2::new(
            (canvas_rect.width() - margin.x * 2.0).max(220.0),
            (canvas_rect.height() - margin.y * 2.0).max(180.0),
        );

        let content_size = content_bounds.size();
        let target_zoom = (available_size.x / content_size.x)
            .min(available_size.y / content_size.y)
            .clamp(min_zoom, 2.5);

        self.zoom = target_zoom;
        self.pan_offset = canvas_rect.center().to_vec2() - content_bounds.center().to_vec2() * target_zoom;
        self.pending_fit_at = None;
    }

    fn fit_board_bounds(&self) -> Option<Rect> {
        self.layout_bounds()
    }

    fn layout_bounds(&self) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;

        for workspace in &self.board.workspaces {
            let rect = self.workspace_canvas_bounds(workspace);
            bounds = Some(match bounds {
                Some(current) => current.union(rect),
                None => rect,
            });
        }

        for (index, panel) in self.board.panels.iter().enumerate() {
            if panel.workspace_id.is_some() {
                continue;
            }
            let position = self
                .panel_canvas_position(panel.id)
                .unwrap_or_else(|| Self::default_panel_canvas_pos(index));
            let rect = Rect::from_min_size(position, Vec2::new(panel.layout.size[0], panel.layout.size[1]));
            bounds = Some(match bounds {
                Some(current) => current.union(rect),
                None => rect,
            });
        }

        bounds
    }

    fn handle_viewport_resize(&mut self, ctx: &Context) {
        let viewport_size = viewport_local_rect(ctx).size();
        if let Some(last_viewport_size) = self.last_viewport_size.replace(viewport_size) {
            let width_changed = (last_viewport_size.x - viewport_size.x).abs() > f32::EPSILON;
            let height_changed = (last_viewport_size.y - viewport_size.y).abs() > f32::EPSILON;
            if width_changed || height_changed {
                self.schedule_fit_board(AUTO_BOARD_RESIZE_SETTLE_DELAY);
            }
        }
    }

    fn maybe_apply_pending_fit(&mut self, ctx: &Context) {
        let Some(run_at) = self.pending_fit_at else {
            return;
        };
        if Instant::now() < run_at {
            return;
        }

        self.fit_view_to_content(ctx);
    }
}

impl eframe::App for OrbitermApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }
        self.handle_zoom(ctx);
        self.handle_canvas_pan(ctx);
        self.handle_canvas_workspace_creation(ctx);
        self.handle_viewport_resize(ctx);
        handle_edge_resize(ctx);

        self.board.process_output();

        let mut closed_any_panels = false;
        for panel_id in self.panels_to_close.drain(..) {
            self.board.close_panel(panel_id);
            self.panel_canvas_rects.remove(&panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.panel_connection_points.remove(&panel_id);
            closed_any_panels = true;
        }

        if closed_any_panels {
            self.remove_empty_workspaces();
            self.schedule_fit_board(Duration::ZERO);
        }

        self.handle_shortcuts(ctx);
        self.check_config_reload();

        paint_root_backdrop(ctx);
        self.render_titlebar(ctx);
        self.render_toolbar(ctx);
        self.render_canvas(ctx);
        self.panel_screen_rects.clear();
        self.panel_canvas_rects.clear();
        self.panel_connection_points.clear();
        self.render_island_backgrounds(ctx);
        self.render_workspace_badges(ctx);
        self.render_preview_panels(ctx);
        self.render_live_panels(ctx);
        self.maybe_apply_pending_fit(ctx);

        self.draw_connectors(ctx);
        render_viewport_resize_handles(ctx);

        if self.show_config_editor {
            self.render_config_editor(ctx);
        }

        ctx.request_repaint();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

fn paint_root_backdrop(ctx: &Context) {
    ctx.layer_painter(LayerId::background())
        .rect_filled(viewport_local_rect(ctx), Rounding::ZERO, theme::BG);
}

fn viewport_local_rect(ctx: &Context) -> Rect {
    ctx.input(|input| input.viewport().inner_rect.or(input.viewport().outer_rect))
        .map_or_else(
            || {
                let rect = ctx.screen_rect();
                Rect::from_min_size(Pos2::ZERO, rect.size())
            },
            |rect| Rect::from_min_size(Pos2::ZERO, rect.size()),
        )
}

fn handle_edge_resize(ctx: &Context) {
    let rect = viewport_local_rect(ctx);
    let Some(pointer_position) = ctx.input(|input| input.pointer.hover_pos()) else {
        return;
    };

    let edge_margin = 6.0;
    let corner_handle_size = 24.0;
    let handle_bottom = rect.max.y - corner_handle_size;
    let left = pointer_position.x - rect.min.x <= edge_margin;
    let right = rect.max.x - pointer_position.x <= edge_margin;
    let top = pointer_position.y - rect.min.y <= edge_margin;
    let bottom = rect.max.y - pointer_position.y <= edge_margin;

    let north_west_handle = Rect::from_min_size(rect.min, Vec2::splat(corner_handle_size));
    let north_east_handle = Rect::from_min_size(
        Pos2::new(rect.max.x - corner_handle_size, rect.min.y),
        Vec2::splat(corner_handle_size),
    );
    let south_west_handle = Rect::from_min_size(Pos2::new(rect.min.x, handle_bottom), Vec2::splat(corner_handle_size));
    let south_east_handle = Rect::from_min_size(
        Pos2::new(rect.max.x - corner_handle_size, handle_bottom),
        Vec2::splat(corner_handle_size),
    );

    let direction = if north_west_handle.contains(pointer_position) {
        Some(egui::ResizeDirection::NorthWest)
    } else if north_east_handle.contains(pointer_position) {
        Some(egui::ResizeDirection::NorthEast)
    } else if south_west_handle.contains(pointer_position) {
        Some(egui::ResizeDirection::SouthWest)
    } else if south_east_handle.contains(pointer_position) {
        Some(egui::ResizeDirection::SouthEast)
    } else if top {
        Some(egui::ResizeDirection::North)
    } else if bottom {
        Some(egui::ResizeDirection::South)
    } else if left {
        Some(egui::ResizeDirection::West)
    } else if right {
        Some(egui::ResizeDirection::East)
    } else {
        None
    };

    if let Some(direction) = direction {
        let cursor_icon = match direction {
            egui::ResizeDirection::East | egui::ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
            egui::ResizeDirection::North | egui::ResizeDirection::South => egui::CursorIcon::ResizeVertical,
            egui::ResizeDirection::NorthWest | egui::ResizeDirection::SouthEast => egui::CursorIcon::ResizeNwSe,
            egui::ResizeDirection::NorthEast | egui::ResizeDirection::SouthWest => egui::CursorIcon::ResizeNeSw,
        };

        ctx.set_cursor_icon(cursor_icon);

        if ctx.input(|input| input.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}

fn paint_status_pill(ui: &mut egui::Ui, text: &str) {
    egui::Frame::default()
        .fill(theme::alpha(theme::PANEL_BG_ALT, 180))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(7.0, 2.0))
        .stroke(Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE, 120)))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(theme::FG_DIM).size(10.0));
        });
}

fn native_titlebar_controls_spec() -> TitlebarControlsSpec {
    if cfg!(target_os = "macos") {
        titlebar_controls_spec_for_platform("macos")
    } else if cfg!(target_os = "windows") {
        titlebar_controls_spec_for_platform("windows")
    } else {
        titlebar_controls_spec_for_platform("linux")
    }
}

fn titlebar_controls_spec_for_platform(platform: &str) -> TitlebarControlsSpec {
    if platform == "macos" {
        TitlebarControlsSpec {
            placement: TitlebarControlsPlacement::Leading,
            controls: [
                TitlebarControl::Close,
                TitlebarControl::Minimize,
                TitlebarControl::Maximize,
            ],
        }
    } else {
        TitlebarControlsSpec {
            placement: TitlebarControlsPlacement::Trailing,
            controls: [
                TitlebarControl::Close,
                TitlebarControl::Maximize,
                TitlebarControl::Minimize,
            ],
        }
    }
}

fn render_titlebar_window_controls(ui: &mut egui::Ui, ctx: &Context, controls: [TitlebarControl; 3]) {
    for control in controls {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::click());
        let fill = if response.hovered() {
            titlebar_control_color(control)
        } else {
            titlebar_control_color(control).gamma_multiply(0.50)
        };
        ui.painter().circle_filled(rect.center(), 6.0, fill);

        if response.clicked() {
            handle_titlebar_control(ctx, control);
        }

        ui.add_space(4.0);
    }
}

fn titlebar_control_color(control: TitlebarControl) -> Color32 {
    match control {
        TitlebarControl::Close => theme::BTN_CLOSE,
        TitlebarControl::Minimize => theme::BTN_MINIMIZE,
        TitlebarControl::Maximize => theme::BTN_MAXIMIZE,
    }
}

fn handle_titlebar_control(ctx: &Context, control: TitlebarControl) {
    match control {
        TitlebarControl::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        TitlebarControl::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
        TitlebarControl::Maximize => {
            let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }
}

fn paint_panel_preview(
    ui: &mut egui::Ui,
    rect: Rect,
    title: &str,
    lines: &[String],
    accent: Color32,
    focused: bool,
    zoom: f32,
) {
    let painter = ui.painter_at(rect);
    let title_height = (26.0 * zoom).clamp(18.0, 30.0);
    let title_font = (12.5 * zoom).clamp(9.0, 13.5);
    let body_font = (11.5 * zoom).clamp(7.5, 12.0);
    let line_step = (13.5 * zoom).clamp(10.0, 15.0);
    let padding = (12.0 * zoom).clamp(8.0, 14.0);

    painter.rect_filled(rect, Rounding::same(14.0), theme::PANEL_BG);
    painter.rect_stroke(
        rect,
        Rounding::same(14.0),
        Stroke::new(1.0, theme::panel_border(accent, focused)),
    );

    let title_rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + title_height));
    painter.rect_filled(
        title_rect,
        Rounding::same(14.0),
        theme::blend(theme::PANEL_BG_ALT, accent, 0.20),
    );
    painter.text(
        Pos2::new(title_rect.min.x + padding, title_rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(title_font),
        theme::FG,
    );

    for (index, line) in lines.iter().enumerate() {
        let y = title_rect.max.y + padding + usize_to_f32(index) * line_step;
        if y > rect.max.y - padding {
            break;
        }
        painter.text(
            Pos2::new(rect.min.x + padding, y),
            egui::Align2::LEFT_TOP,
            line,
            egui::FontId::monospace(body_font),
            theme::alpha(theme::FG_SOFT, 220),
        );
    }
}

fn paint_workspace_title_pill(
    ui: &mut egui::Ui,
    title: &str,
    accent: Color32,
    text_size: f32,
    width_bounds: [f32; 2],
) -> Rect {
    let [min_width, max_width] = width_bounds;
    let desired_width = (usize_to_f32(title.chars().count().max(8)) * (text_size * 0.62)).clamp(min_width, max_width);
    let response = egui::Frame::default()
        .fill(theme::alpha(accent, 34))
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(12.0, 4.0))
        .stroke(Stroke::new(1.0, theme::alpha(accent, 120)))
        .show(ui, |ui| {
            ui.add_sized(
                [desired_width, text_size + 4.0],
                egui::Label::new(egui::RichText::new(title).color(theme::FG).size(text_size).strong()).truncate(),
            )
        });

    response.response.rect
}

fn render_pty_controls(ui: &mut egui::Ui, panel: &mut orbiterm_core::Panel) -> bool {
    let mut pty_resized = false;

    ui.horizontal(|ui| {
        let auto_label = if panel.auto_resize_pty {
            "Auto PTY"
        } else {
            "Manual PTY"
        };
        let toggle_button = if panel.auto_resize_pty {
            chrome_button(auto_label)
        } else {
            primary_button(auto_label)
        };
        if ui.add(toggle_button).clicked() {
            panel.set_auto_resize_pty(!panel.auto_resize_pty);
        }

        ui.label(
            egui::RichText::new(format!("{}x{}", panel.terminal.cols(), panel.terminal.rows()))
                .color(theme::FG_DIM)
                .size(10.5),
        );
        ui.label(
            egui::RichText::new(format!(
                "scroll {}/{}",
                panel.terminal.scrollback(),
                panel.terminal.scrollback_limit()
            ))
            .color(theme::FG_DIM)
            .size(10.5),
        );

        if ui.add(icon_button("R-")).on_hover_text("Decrease PTY rows").clicked() {
            panel.adjust_pty_size(-1, 0);
            pty_resized = true;
        }
        if ui.add(icon_button("R+")).on_hover_text("Increase PTY rows").clicked() {
            panel.adjust_pty_size(1, 0);
            pty_resized = true;
        }
        if ui
            .add(icon_button("C-"))
            .on_hover_text("Decrease PTY columns")
            .clicked()
        {
            panel.adjust_pty_size(0, -1);
            pty_resized = true;
        }
        if ui
            .add(icon_button("C+"))
            .on_hover_text("Increase PTY columns")
            .clicked()
        {
            panel.adjust_pty_size(0, 1);
            pty_resized = true;
        }
    });

    pty_resized
}

fn preview_lines(panel: &orbiterm_core::Panel, max_lines: usize, max_chars: usize) -> Vec<String> {
    let screen = panel.terminal.screen();
    let (_, cols) = screen.size();
    let width = cols.min(u16::try_from(max_chars).unwrap_or(u16::MAX));
    let rows: Vec<String> = screen.rows(0, width).map(|row| row.trim_end().to_string()).collect();
    let last_content_row = rows
        .iter()
        .rposition(|row| !row.is_empty())
        .map_or(0, |index| index + 1);
    let start = last_content_row.saturating_sub(max_lines);
    let selected = rows
        .into_iter()
        .skip(start)
        .take(max_lines)
        .map(|row| truncate_preview_line(row, max_chars))
        .collect::<Vec<_>>();

    if selected.iter().all(String::is_empty) {
        vec!["terminal idle".to_string()]
    } else {
        selected
    }
}

fn truncate_preview_line(mut line: String, max_chars: usize) -> String {
    if line.chars().count() <= max_chars || max_chars <= 3 {
        return line;
    }

    line = line.chars().take(max_chars - 3).collect();
    line.push_str("...");
    line
}

fn render_viewport_resize_handles(ctx: &Context) {
    let rect = viewport_local_rect(ctx);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("viewport_handles")));
    let stroke = Stroke::new(1.0, theme::alpha(theme::VIEWPORT_HANDLE, 180));
    let inset = 8.0;
    let size = 14.0;

    paint_corner_bracket(
        &painter,
        Pos2::new(rect.min.x + inset, rect.min.y + inset),
        stroke,
        size,
        1.0,
        1.0,
    );
    paint_corner_bracket(
        &painter,
        Pos2::new(rect.max.x - inset, rect.min.y + inset),
        stroke,
        size,
        -1.0,
        1.0,
    );
    paint_corner_bracket(
        &painter,
        Pos2::new(rect.min.x + inset, rect.max.y - inset),
        stroke,
        size,
        1.0,
        -1.0,
    );
    paint_corner_bracket(
        &painter,
        Pos2::new(rect.max.x - inset, rect.max.y - inset),
        stroke,
        size,
        -1.0,
        -1.0,
    );
}

fn paint_corner_bracket(
    painter: &egui::Painter,
    corner: Pos2,
    stroke: Stroke,
    size: f32,
    x_direction: f32,
    y_direction: f32,
) {
    painter.line_segment([corner, corner + Vec2::new(size * x_direction, 0.0)], stroke);
    painter.line_segment([corner, corner + Vec2::new(0.0, size * y_direction)], stroke);
}

fn draw_dot_grid(ui: &mut egui::Ui, pan_offset: Vec2, zoom: f32) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let spacing = (22.0 * zoom).clamp(14.0, 52.0);
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

    let highlight = Rect::from_min_max(
        Pos2::new(rect.max.x - 6.0, rect.min.y),
        Pos2::new(rect.max.x, rect.max.y),
    );
    painter.rect_filled(highlight, Rounding::ZERO, theme::alpha(theme::ACCENT_WARM, 110));
}

fn paint_workspace_dot(ui: &mut egui::Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}

fn render_count_badge(ui: &mut egui::Ui, accent: Color32, count: usize) {
    egui::Frame::default()
        .fill(theme::alpha(accent, 32))
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(count.to_string())
                    .color(theme::alpha(accent, 220))
                    .size(10.0),
            );
        });
}

fn should_create_workspace_from_canvas_gesture(
    triggered: bool,
    canvas_rect: Rect,
    pointer_position: Pos2,
    panel_rects: &[Rect],
    workspace_rects: &[Rect],
) -> bool {
    triggered
        && canvas_rect.contains(pointer_position)
        && panel_rects.iter().all(|rect| !rect.contains(pointer_position))
        && workspace_rects.iter().all(|rect| !rect.contains(pointer_position))
}

fn clamp_rect_to_bounds(rect: Rect, bounds: Rect) -> Rect {
    let size = Vec2::new(rect.width().min(bounds.width()), rect.height().min(bounds.height()));
    let max_min_x = bounds.max.x - size.x;
    let max_min_y = bounds.max.y - size.y;
    let min = Pos2::new(
        rect.min.x.clamp(bounds.min.x, max_min_x),
        rect.min.y.clamp(bounds.min.y, max_min_y),
    );

    Rect::from_min_size(min, size)
}

fn workspace_name_from_input(input: &str, fallback_name: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        fallback_name.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn agent_panel_defaults(kind: PanelKind) -> (&'static str, PanelResume, bool) {
    match kind {
        PanelKind::Codex => ("codex", PanelResume::Last, false),
        PanelKind::Claude => ("claude", PanelResume::Last, false),
        _ => ("shell", PanelResume::Fresh, false),
    }
}

#[cfg(test)]
fn panel_size_changed(previous_size: Vec2, current_size: Vec2) -> bool {
    const RESIZE_EPSILON: f32 = 1.0;

    (current_size.x - previous_size.x).abs() > RESIZE_EPSILON
        || (current_size.y - previous_size.y).abs() > RESIZE_EPSILON
}

fn preview_char_budget(screen_width: f32) -> usize {
    let char_slots = ((screen_width - 24.0) / 7.0).floor().clamp(12.0, 80.0);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        char_slots as usize
    }
}

fn primary_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.5).color(theme::FG))
        .fill(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28))
        .stroke(Stroke::new(
            1.0,
            theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.72),
        ))
        .rounding(Rounding::same(10.0))
}

fn chrome_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(11.0).color(theme::FG_SOFT))
        .fill(theme::PANEL_BG_ALT)
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 210)))
        .rounding(Rounding::same(10.0))
}

fn icon_button(text: &str) -> Button<'_> {
    Button::new(egui::RichText::new(text).size(12.0).color(theme::FG_SOFT))
        .min_size(Vec2::splat(20.0))
        .fill(theme::alpha(theme::PANEL_BG_ALT, 220))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 190)))
        .rounding(Rounding::same(999.0))
}

fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::{
        TitlebarControl, TitlebarControlsPlacement, agent_panel_defaults, clamp_rect_to_bounds, panel_size_changed,
        preview_char_budget, should_create_workspace_from_canvas_gesture, titlebar_controls_spec_for_platform,
        workspace_name_from_input,
    };
    use egui::{Pos2, Rect, Vec2};
    use orbiterm_core::{PanelKind, PanelResume};

    #[test]
    fn workspace_creation_gesture_only_fires_on_empty_canvas() {
        let canvas = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(800.0, 500.0));
        let panel_rects = [Rect::from_min_size(Pos2::new(200.0, 120.0), Vec2::new(240.0, 180.0))];
        let workspace_rects = [Rect::from_min_size(Pos2::new(80.0, 72.0), Vec2::new(220.0, 52.0))];

        assert!(should_create_workspace_from_canvas_gesture(
            true,
            canvas,
            Pos2::new(520.0, 240.0),
            &panel_rects,
            &workspace_rects,
        ));
        assert!(!should_create_workspace_from_canvas_gesture(
            true,
            canvas,
            Pos2::new(260.0, 160.0),
            &panel_rects,
            &workspace_rects,
        ));
        assert!(!should_create_workspace_from_canvas_gesture(
            true,
            canvas,
            Pos2::new(120.0, 90.0),
            &panel_rects,
            &workspace_rects,
        ));
        assert!(!should_create_workspace_from_canvas_gesture(
            false,
            canvas,
            Pos2::new(520.0, 240.0),
            &panel_rects,
            &workspace_rects,
        ));
    }

    #[test]
    fn clamped_rect_stays_inside_bounds() {
        let bounds = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(600.0, 320.0));
        let rect = Rect::from_min_size(Pos2::new(-80.0, 0.0), Vec2::new(720.0, 400.0));

        let clamped = clamp_rect_to_bounds(rect, bounds);

        assert_eq!(clamped.min, bounds.min);
        assert_eq!(clamped.max, bounds.max);
    }

    #[test]
    fn workspace_name_defaults_when_input_is_blank() {
        assert_eq!(workspace_name_from_input("   ", "workspace 4"), "workspace 4");
        assert_eq!(workspace_name_from_input(" agents ", "workspace 4"), "agents");
    }

    #[test]
    fn preview_char_budget_stays_within_expected_bounds() {
        assert_eq!(preview_char_budget(20.0), 12);
        assert_eq!(preview_char_budget(400.0), 53);
        assert_eq!(preview_char_budget(900.0), 80);
    }

    #[test]
    fn agent_panels_start_with_manual_pty() {
        assert_eq!(
            agent_panel_defaults(PanelKind::Codex),
            ("codex", PanelResume::Last, false)
        );
        assert_eq!(
            agent_panel_defaults(PanelKind::Claude),
            ("claude", PanelResume::Last, false)
        );
        assert_eq!(
            agent_panel_defaults(PanelKind::Shell),
            ("shell", PanelResume::Fresh, false)
        );
    }

    #[test]
    fn panel_resize_detection_is_symmetric() {
        let previous_size = Vec2::new(280.0, 200.0);
        let current_size = Vec2::new(320.0, 220.0);
        let smaller_size = Vec2::new(240.0, 180.0);
        let nearly_same = Vec2::new(280.4, 200.4);

        assert!(panel_size_changed(previous_size, current_size));
        assert!(panel_size_changed(previous_size, smaller_size));
        assert!(!panel_size_changed(previous_size, nearly_same));
    }

    #[test]
    fn titlebar_controls_follow_platform_conventions() {
        let linux = titlebar_controls_spec_for_platform("linux");
        assert_eq!(linux.placement, TitlebarControlsPlacement::Trailing);
        assert_eq!(
            linux.controls,
            [
                TitlebarControl::Close,
                TitlebarControl::Maximize,
                TitlebarControl::Minimize,
            ]
        );

        let windows = titlebar_controls_spec_for_platform("windows");
        assert_eq!(windows.placement, TitlebarControlsPlacement::Trailing);
        assert_eq!(
            windows.controls,
            [
                TitlebarControl::Close,
                TitlebarControl::Maximize,
                TitlebarControl::Minimize,
            ]
        );

        let macos = titlebar_controls_spec_for_platform("macos");
        assert_eq!(macos.placement, TitlebarControlsPlacement::Leading);
        assert_eq!(
            macos.controls,
            [
                TitlebarControl::Close,
                TitlebarControl::Minimize,
                TitlebarControl::Maximize,
            ]
        );
    }
}
