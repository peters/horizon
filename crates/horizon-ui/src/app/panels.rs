use egui::{
    Align, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder,
    Vec2,
};
use horizon_core::{AgentSessionBinding, AttentionSeverity, Panel, PanelId, PanelKind, WorkspaceId};

use crate::editor_widget::MarkdownEditorView;
use crate::git_changes_widget::GitChangesView;
use crate::terminal_widget::TerminalView;
use crate::theme;
use crate::usage_widget::UsageDashboardView;

use super::util::{clamp_panel_size, format_compact_count, usize_to_f32};
use super::{HorizonApp, PANEL_PADDING, PANEL_TITLEBAR_HEIGHT, RESIZE_HANDLE_SIZE, RenameEditAction};

struct PanelSnapshot {
    screen_rect: Rect,
    canvas_position: Pos2,
    canvas_size: Vec2,
    current_workspace_id: WorkspaceId,
    title: String,
    kind: PanelKind,
    history_size: usize,
    scrollback_limit: usize,
    workspace_accent: Option<Color32>,
    is_focused: bool,
    is_renaming: bool,
    rebind_options: Vec<(String, AgentSessionBinding)>,
    attention_badge: Option<(AttentionSeverity, String)>,
}

#[derive(Default)]
struct PanelUiOutcome {
    focus_requested: bool,
    drag_delta: Vec2,
    resize_delta: Vec2,
    workspace_assignment: Option<WorkspaceId>,
    command: Option<PanelCommand>,
    rename_action: RenameEditAction,
}

#[derive(Clone, Copy)]
enum PanelCommand {
    Close,
    CreateWorkspace,
    StartRename,
}

struct PanelFrame {
    panel: Rect,
    titlebar: Rect,
    body: Rect,
    close: Rect,
    resize: Rect,
}

impl PanelFrame {
    fn new(panel_rect: Rect) -> Self {
        let titlebar = Rect::from_min_max(
            panel_rect.min,
            Pos2::new(panel_rect.max.x, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT),
        );
        let body = Rect::from_min_max(
            Pos2::new(panel_rect.min.x + PANEL_PADDING, titlebar.max.y + PANEL_PADDING),
            Pos2::new(panel_rect.max.x - PANEL_PADDING, panel_rect.max.y - PANEL_PADDING),
        );
        let close = Rect::from_center_size(
            Pos2::new(panel_rect.max.x - 18.0, panel_rect.min.y + PANEL_TITLEBAR_HEIGHT * 0.5),
            Vec2::splat(16.0),
        );
        let resize = Rect::from_min_size(
            Pos2::new(
                panel_rect.max.x - RESIZE_HANDLE_SIZE,
                panel_rect.max.y - RESIZE_HANDLE_SIZE,
            ),
            Vec2::splat(RESIZE_HANDLE_SIZE),
        );

        Self {
            panel: panel_rect,
            titlebar,
            body,
            close,
            resize,
        }
    }
}

fn show_panel_body_contents(ui: &mut egui::Ui, panel: &mut Panel, is_focused: bool) -> bool {
    match panel.kind {
        PanelKind::Editor => MarkdownEditorView::new(panel).show(ui, is_focused),
        PanelKind::GitChanges => GitChangesView::new(panel).show(ui, is_focused),
        PanelKind::Usage => UsageDashboardView::new(panel).show(ui, is_focused),
        _ => TerminalView::new(panel).show(ui, is_focused),
    }
}

impl HorizonApp {
    #[profiling::function]
    pub(super) fn render_fullscreen_panel(&mut self, ctx: &Context) {
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
                            show_panel_body_contents(ui, panel, true);
                        }
                    },
                );
            });
    }

    #[profiling::function]
    pub(super) fn render_panels(&mut self, ctx: &Context) {
        self.panel_screen_rects.clear();

        let workspaces: Vec<(WorkspaceId, String, Color32)> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                (workspace.id, workspace.name.clone(), Color32::from_rgb(r, g, b))
            })
            .collect();

        let mut panel_order: Vec<_> = self
            .board
            .panels
            .iter()
            .enumerate()
            .map(|(index, panel)| (panel.id, index))
            .collect();
        let focused = self.board.focused;
        panel_order.sort_by_key(|(panel_id, _)| Some(*panel_id) == focused);

        let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
        let mut panels_to_close = Vec::new();

        for (panel_id, fallback_index) in panel_order {
            if self.render_panel(ctx, canvas_rect, panel_id, fallback_index, &workspaces) {
                panels_to_close.push(panel_id);
            }
        }

        self.panels_to_close = panels_to_close;
    }

    #[profiling::function]
    pub(super) fn render_panel(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        panel_id: PanelId,
        _fallback_index: usize,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> bool {
        let Some(snapshot) = self.panel_snapshot(panel_id, canvas_rect, workspaces) else {
            return false;
        };
        let outcome = self.show_panel_area(ctx, panel_id, &snapshot, workspaces);
        self.apply_panel_outcome(panel_id, &snapshot, &outcome)
    }

    #[profiling::function]
    fn panel_snapshot(
        &self,
        panel_id: PanelId,
        canvas_rect: Rect,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> Option<PanelSnapshot> {
        self.board.panel(panel_id).and_then(|panel| {
            let terminal = panel.terminal();
            let canvas_position = Pos2::new(panel.layout.position[0], panel.layout.position[1]);
            let canvas_size = Vec2::new(panel.layout.size[0], panel.layout.size[1]);
            let screen_rect = Rect::from_min_size(self.canvas_to_screen(canvas_rect, canvas_position), canvas_size);

            // Cull off-screen panels — skip chrome, snapshot, and rendering.
            if !canvas_rect.intersects(screen_rect) {
                return None;
            }

            let workspace_accent = workspaces
                .iter()
                .find(|(workspace_id, _, _)| *workspace_id == panel.workspace_id)
                .map(|(_, _, color)| *color);

            let attention_badge = if self.template_config.features.attention_feed {
                self.board
                    .unresolved_attention_for_panel(panel_id)
                    .map(|item| (item.severity, item.summary.clone()))
            } else {
                None
            };

            Some(PanelSnapshot {
                screen_rect,
                canvas_position,
                canvas_size,
                current_workspace_id: panel.workspace_id,
                title: panel.title.clone(),
                kind: panel.kind,
                history_size: terminal.map_or(0, horizon_core::Terminal::history_size),
                scrollback_limit: terminal.map_or(0, horizon_core::Terminal::scrollback_limit),
                workspace_accent,
                is_focused: self.board.focused == Some(panel_id),
                is_renaming: self.renaming_panel == Some(panel_id),
                rebind_options: self.session_rebind_options(panel_id),
                attention_badge,
            })
        })
    }

    #[profiling::function]
    fn show_panel_area(
        &mut self,
        ctx: &Context,
        panel_id: PanelId,
        snapshot: &PanelSnapshot,
        workspaces: &[(WorkspaceId, String, Color32)],
    ) -> PanelUiOutcome {
        let mut outcome = PanelUiOutcome::default();

        egui::Area::new(Id::new(("panel", panel_id.0)))
            .fixed_pos(snapshot.screen_rect.min)
            .constrain(false)
            .order(if snapshot.is_focused {
                Order::Foreground
            } else {
                Order::Middle
            })
            .show(ctx, |ui| {
                let (panel_rect, _) = ui.allocate_exact_size(snapshot.screen_rect.size(), Sense::hover());
                let rects = PanelFrame::new(panel_rect);
                let drag_response = ui.interact(
                    rects.titlebar,
                    ui.make_persistent_id(("panel_drag", panel_id.0)),
                    if snapshot.is_renaming {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                let close_response = ui.interact(
                    rects.close.expand2(Vec2::splat(4.0)),
                    ui.make_persistent_id(("panel_close", panel_id.0)),
                    Sense::click(),
                );
                let resize_response = ui.interact(
                    rects.resize.expand2(Vec2::splat(6.0)),
                    ui.make_persistent_id(("panel_resize", panel_id.0)),
                    Sense::click_and_drag(),
                );

                Self::update_panel_interactions(
                    ctx,
                    snapshot.is_renaming,
                    &drag_response,
                    &close_response,
                    &resize_response,
                    &mut outcome,
                );
                if !snapshot.is_renaming {
                    self.show_panel_context_menu(&drag_response, panel_id, snapshot, workspaces, &mut outcome);
                }

                paint_panel_chrome(
                    ui,
                    panel_id,
                    rects.panel,
                    rects.titlebar,
                    rects.close,
                    rects.resize,
                    if snapshot.is_renaming {
                        None
                    } else {
                        Some(snapshot.title.as_str())
                    },
                    snapshot.history_size,
                    snapshot.scrollback_limit,
                    snapshot.is_focused,
                    close_response.hovered(),
                    snapshot.workspace_accent,
                    snapshot.attention_badge.as_ref(),
                );

                if snapshot.is_renaming {
                    outcome.rename_action = show_inline_rename_editor(
                        ui,
                        panel_title_content_rect(rects.titlebar, rects.close, snapshot.workspace_accent.is_some()),
                        &mut self.panel_rename_buffer,
                        egui::FontId::proportional(13.0),
                    );
                }

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rects.body)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        if let Some(panel) = self.board.panel_mut(panel_id) {
                            outcome.focus_requested |= show_panel_body_contents(ui, panel, snapshot.is_focused);
                        }
                    },
                );
            });

        outcome
    }

    fn update_panel_interactions(
        ctx: &Context,
        is_renaming: bool,
        drag_response: &egui::Response,
        close_response: &egui::Response,
        resize_response: &egui::Response,
        outcome: &mut PanelUiOutcome,
    ) {
        if resize_response.drag_started() || resize_response.clicked() {
            outcome.focus_requested = true;
        }
        if !is_renaming && (drag_response.clicked() || drag_response.drag_started()) {
            outcome.focus_requested = true;
        }
        if !is_renaming && drag_response.dragged() {
            outcome.drag_delta = ctx.input(|input| input.pointer.delta());
        }
        if resize_response.dragged() {
            outcome.resize_delta = ctx.input(|input| input.pointer.delta());
        }
        if close_response.clicked() {
            outcome.command = Some(PanelCommand::Close);
        }
        if !is_renaming && drag_response.double_clicked() {
            outcome.command = Some(PanelCommand::StartRename);
            outcome.focus_requested = true;
        }
    }

    fn show_panel_context_menu(
        &mut self,
        drag_response: &egui::Response,
        panel_id: PanelId,
        snapshot: &PanelSnapshot,
        workspaces: &[(WorkspaceId, String, Color32)],
        outcome: &mut PanelUiOutcome,
    ) {
        drag_response.context_menu(|ui| {
            ui.set_min_width(180.0);
            ui.label(egui::RichText::new("Move to Workspace").size(11.0).color(theme::FG_DIM));
            ui.separator();

            for (workspace_id, workspace_name, workspace_color) in workspaces {
                let is_current = snapshot.current_workspace_id == *workspace_id;
                let label = if is_current {
                    format!("● {workspace_name}")
                } else {
                    format!("  {workspace_name}")
                };
                let text = egui::RichText::new(label)
                    .color(if is_current { *workspace_color } else { theme::FG_SOFT })
                    .size(12.0);
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    outcome.workspace_assignment = Some(*workspace_id);
                    ui.close();
                }
            }

            ui.separator();
            if !snapshot.rebind_options.is_empty() {
                ui.menu_button("Rebind Session", |ui| {
                    ui.set_min_width(220.0);
                    for (label, binding) in &snapshot.rebind_options {
                        let button =
                            egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::FG_SOFT)).frame(false);
                        if ui.add(button).clicked() {
                            self.pending_session_rebinds.push((panel_id, binding.clone()));
                            ui.close();
                        }
                    }
                });
                ui.separator();
            }
            if ui.button("New Workspace").clicked() {
                outcome.command = Some(PanelCommand::CreateWorkspace);
                ui.close();
            }
            if snapshot.kind.is_agent() {
                ui.separator();
                if ui.button("Restart").clicked() {
                    self.panels_to_restart.push(panel_id);
                    ui.close();
                }
            }
        });
    }

    fn apply_panel_outcome(&mut self, panel_id: PanelId, snapshot: &PanelSnapshot, outcome: &PanelUiOutcome) -> bool {
        self.panel_screen_rects.insert(panel_id, snapshot.screen_rect);

        if matches!(outcome.command, Some(PanelCommand::StartRename)) {
            self.clear_workspace_rename();
            self.renaming_panel = Some(panel_id);
            self.panel_rename_buffer.clone_from(&snapshot.title);
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

        if !self.is_panning && outcome.drag_delta != Vec2::ZERO {
            let new_position = snapshot.canvas_position + outcome.drag_delta;
            let _ = self.board.move_panel(panel_id, [new_position.x, new_position.y]);
            self.mark_runtime_dirty();
        }
        if !self.is_panning && outcome.resize_delta != Vec2::ZERO {
            let new_size = clamp_panel_size(snapshot.canvas_size + outcome.resize_delta);
            let _ = self.board.resize_panel(panel_id, [new_size.x, new_size.y]);
            self.mark_runtime_dirty();
        }
        if outcome.focus_requested {
            self.board.focus(panel_id);
        }
        if matches!(outcome.command, Some(PanelCommand::CreateWorkspace)) {
            self.workspace_creates.push(panel_id);
        }
        if let Some(workspace_id) = outcome.workspace_assignment {
            self.workspace_assignments.push((panel_id, workspace_id));
        }

        matches!(outcome.command, Some(PanelCommand::Close))
    }
}

pub(super) fn panel_kind_icon(kind: PanelKind, workspace_color: Color32, focused: bool) -> (&'static str, Color32) {
    match kind {
        PanelKind::Shell | PanelKind::Command => (">_", theme::alpha(workspace_color, if focused { 200 } else { 80 })),
        PanelKind::Codex => (
            "CX",
            theme::alpha(Color32::from_rgb(116, 162, 247), if focused { 220 } else { 120 }),
        ),
        PanelKind::Claude => (
            "CC",
            theme::alpha(Color32::from_rgb(203, 166, 247), if focused { 220 } else { 120 }),
        ),
        PanelKind::Editor => (
            "MD",
            theme::alpha(Color32::from_rgb(166, 227, 161), if focused { 220 } else { 120 }),
        ),
        PanelKind::GitChanges => (
            "GC",
            theme::alpha(Color32::from_rgb(249, 226, 175), if focused { 220 } else { 120 }),
        ),
        PanelKind::Usage => (
            "US",
            theme::alpha(Color32::from_rgb(233, 190, 109), if focused { 220 } else { 120 }),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_panel_chrome(
    ui: &mut egui::Ui,
    panel_id: PanelId,
    panel_rect: Rect,
    titlebar_rect: Rect,
    close_rect: Rect,
    resize_rect: Rect,
    title: Option<&str>,
    history_size: usize,
    scrollback_limit: usize,
    focused: bool,
    close_hovered: bool,
    workspace_accent: Option<Color32>,
    attention_badge: Option<&(AttentionSeverity, String)>,
) {
    let painter = ui.painter_at(panel_rect);
    let accent = workspace_accent.unwrap_or(if focused { theme::ACCENT } else { theme::BORDER_STRONG });

    painter.rect_filled(panel_rect, CornerRadius::same(16), theme::PANEL_BG);
    painter.rect_stroke(
        panel_rect,
        CornerRadius::same(16),
        Stroke::new(1.2, theme::panel_border(accent, focused)),
        StrokeKind::Outside,
    );
    painter.rect_filled(
        titlebar_rect,
        CornerRadius::same(16),
        theme::blend(theme::PANEL_BG_ALT, accent, if focused { 0.18 } else { 0.10 }),
    );

    if let Some(title) = title {
        let title_x = if let Some(color) = workspace_accent {
            painter.circle_filled(
                Pos2::new(titlebar_rect.min.x + 14.0, titlebar_rect.center().y),
                4.5,
                color,
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
    }

    if let Some((severity, summary)) = attention_badge {
        paint_attention_badge(&painter, titlebar_rect, close_rect, *severity, summary);
    }

    if scrollback_limit > 0 {
        paint_history_meter(
            ui,
            &painter,
            panel_id,
            titlebar_rect,
            close_rect,
            accent,
            history_size,
            scrollback_limit,
            focused,
        );
    }

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

#[allow(clippy::too_many_arguments)]
fn paint_history_meter(
    ui: &egui::Ui,
    painter: &egui::Painter,
    panel_id: PanelId,
    titlebar_rect: Rect,
    close_rect: Rect,
    accent: Color32,
    history_size: usize,
    scrollback_limit: usize,
    focused: bool,
) {
    let badge_rect = panel_history_badge_rect(titlebar_rect, close_rect);
    let track_rect = Rect::from_min_max(
        Pos2::new(badge_rect.min.x + 8.0, badge_rect.max.y - 5.0),
        Pos2::new(badge_rect.max.x - 8.0, badge_rect.max.y - 3.0),
    );
    let ratio = if scrollback_limit == 0 {
        0.0
    } else {
        (usize_to_f32(history_size) / usize_to_f32(scrollback_limit)).clamp(0.0, 1.0)
    };
    let animated_ratio = ui
        .ctx()
        .animate_value_with_time(Id::new(("panel_history_ratio", panel_id.0)), ratio, 0.16);
    let fill_width = track_rect.width() * animated_ratio.clamp(0.0, 1.0);
    let fill_rect = Rect::from_min_max(
        track_rect.min,
        Pos2::new(track_rect.min.x + fill_width, track_rect.max.y),
    );
    let history_text = format!(
        "{}/{}",
        format_compact_count(history_size),
        format_compact_count(scrollback_limit)
    );

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(7),
        theme::alpha(
            theme::blend(theme::BG_ELEVATED, accent, 0.10),
            if focused { 214 } else { 184 },
        ),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(7),
        Stroke::new(1.0, theme::alpha(theme::blend(theme::BORDER_SUBTLE, accent, 0.34), 180)),
        StrokeKind::Outside,
    );
    painter.rect_filled(track_rect, CornerRadius::same(2), theme::alpha(theme::FG_DIM, 52));
    if fill_width > 0.0 {
        painter.rect_filled(
            fill_rect,
            CornerRadius::same(2),
            theme::alpha(
                theme::blend(theme::ACCENT, accent, 0.35),
                if focused { 224 } else { 188 },
            ),
        );
    }
    painter.text(
        Pos2::new(badge_rect.center().x, badge_rect.center().y - 2.0),
        egui::Align2::CENTER_CENTER,
        history_text,
        egui::FontId::monospace(10.5),
        if history_size > 0 {
            theme::FG_SOFT
        } else {
            theme::FG_DIM
        },
    );
}

fn paint_attention_badge(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    severity: AttentionSeverity,
    summary: &str,
) {
    let color = attention_severity_color(severity);
    let icon = attention_severity_icon(severity);

    // Truncate the summary for display.
    let display_text = if summary.len() > 30 {
        let mut truncated = summary[..29].to_string();
        truncated.push('\u{2026}');
        truncated
    } else {
        summary.to_string()
    };
    let badge_text = format!("{icon} {display_text}");
    let font = egui::FontId::proportional(10.0);

    // Position the badge left of the history meter area.
    let history_badge = panel_history_badge_rect(titlebar_rect, close_rect);
    let badge_right = history_badge.min.x - 6.0;
    let text_galley = painter.layout_no_wrap(badge_text.clone(), font.clone(), color);
    let text_width = text_galley.size().x;
    let badge_width = text_width + 12.0;
    let badge_height: f32 = 18.0;
    let badge_left = (badge_right - badge_width).max(titlebar_rect.min.x + 60.0);
    let badge_rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_right - badge_left, badge_height),
    );

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_premultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
    );
    painter.text(
        Pos2::new(badge_left + 6.0, titlebar_rect.center().y),
        egui::Align2::LEFT_CENTER,
        badge_text,
        font,
        color,
    );
}

fn attention_severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED,
        AttentionSeverity::Medium => theme::PALETTE_GREEN,
        AttentionSeverity::Low => theme::ACCENT,
    }
}

fn attention_severity_icon(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "\u{26A0}",
        AttentionSeverity::Medium => "\u{2713}",
        AttentionSeverity::Low => "\u{2139}",
    }
}

fn panel_history_badge_rect(titlebar_rect: Rect, close_rect: Rect) -> Rect {
    let badge_size = Vec2::new(96.0, 20.0);
    Rect::from_center_size(
        Pos2::new(close_rect.min.x - (badge_size.x * 0.5) - 10.0, titlebar_rect.center().y),
        badge_size,
    )
}

pub(super) fn panel_title_content_rect(titlebar_rect: Rect, close_rect: Rect, has_workspace_accent: bool) -> Rect {
    let left = if has_workspace_accent {
        titlebar_rect.min.x + 26.0
    } else {
        titlebar_rect.min.x + 12.0
    };
    let badge_rect = panel_history_badge_rect(titlebar_rect, close_rect);
    let right = (badge_rect.min.x - 12.0).max(left + 1.0);

    Rect::from_min_max(
        Pos2::new(left, titlebar_rect.min.y + 2.0),
        Pos2::new(right, titlebar_rect.max.y - 2.0),
    )
}

pub(super) fn show_inline_rename_editor(
    ui: &mut egui::Ui,
    rect: Rect,
    buffer: &mut String,
    font: egui::FontId,
) -> RenameEditAction {
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let edit = egui::TextEdit::singleline(buffer)
        .font(font)
        .text_color(theme::FG)
        .frame(false)
        .desired_width(rect.width())
        .margin(Margin::ZERO);
    let response = ui.add(edit);
    if !response.has_focus() {
        response.request_focus();
    }

    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let lost_focus = response.lost_focus();

    if escape {
        RenameEditAction::Cancel
    } else if enter || lost_focus {
        RenameEditAction::Commit
    } else {
        RenameEditAction::None
    }
}
