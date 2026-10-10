//! The rows of the sidebar's cloud list: a status dot, the workspace name and one
//! status line, or only the dot and the name for a compact parked row.
use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::cloud_list::{Dot, Group};

use crate::theme;

use super::{SidebarWorkspaceInsert, SidebarWorkspaceRowInteraction, WorkspaceSidebarEntry};

/// The height of a row with a status line.
pub(super) const ROW_HEIGHT: f32 = 40.0;
/// The height of a compact parked row.
pub(super) const COMPACT_ROW_HEIGHT: f32 = 26.0;
const DOT_RADIUS: f32 = 4.0;

/// Whether the row of `workspace` is compact: a parked row shows no status line.
pub(super) fn is_compact(workspace: &WorkspaceSidebarEntry) -> bool {
    workspace.row.group == Group::Parked
}

/// Width left for the workspace name after the badges that follow it: the
/// panel count (accordion rows only) and the `NEW WINDOW` badge (detached
/// rows). Sizing the label to this width lets it truncate instead of running
/// past the sidebar edge.
pub(super) fn sidebar_workspace_name_width(available_width: f32, detached: bool, accordion: bool) -> f32 {
    let count_reserve = if accordion { 28.0 } else { 0.0 };
    // Badge text plus the explicit 4px gap and egui's item spacing before it.
    let detached_reserve = if detached { 76.0 } else { 0.0 };
    (available_width - count_reserve - detached_reserve - 10.0).max(0.0)
}

/// What the status dot says, for its hover text.
pub(super) fn dot_meaning(dot: Dot) -> &'static str {
    match dot {
        Dot::Working => "An agent is working",
        Dot::Idle => "Idle",
        Dot::Busy => "Horizon is working on this cloud",
        Dot::Attention => "Waiting for you",
        Dot::Failed => "Failed",
        Dot::Parked { working: true } => "Parked · an agent is working on the worker",
        Dot::Parked { working: false } => "Parked · no local terminal",
        Dot::Stopped => "Worker stopped",
    }
}

pub(super) fn paint_dot(painter: &egui::Painter, center: Pos2, dot: Dot) {
    let filled = |color: Color32| {
        painter.circle_filled(center, DOT_RADIUS, color);
    };
    let ring = |color: Color32| {
        painter.circle_stroke(center, DOT_RADIUS - 0.5, Stroke::new(1.5_f32, color));
    };
    match dot {
        Dot::Working => filled(theme::PALETTE_GREEN()),
        Dot::Idle => filled(theme::FG_DIM()),
        Dot::Busy => filled(theme::ACCENT()),
        Dot::Attention => filled(theme::PALETTE_YELLOW()),
        Dot::Failed => filled(theme::PALETTE_RED()),
        Dot::Parked { working: true } => ring(theme::PALETTE_GREEN()),
        Dot::Parked { working: false } => ring(theme::FG_DIM()),
        Dot::Stopped => ring(theme::BORDER_STRONG()),
    }
}

/// Draws the dot, the name with its badges and, unless the row is compact, the
/// status line.
pub(super) fn render_sidebar_workspace_row_contents(
    ui: &mut egui::Ui,
    workspace: &WorkspaceSidebarEntry,
    accordion: bool,
) -> SidebarWorkspaceRowInteraction {
    let mut hovered = false;
    let mut clicked = false;
    let mut track = |response: &egui::Response| {
        hovered |= response.hovered();
        clicked |= response.clicked();
    };

    let compact = is_compact(workspace);
    ui.add_space(16.0);
    let (dot_rect, dot_response) = ui.allocate_exact_size(Vec2::splat(DOT_RADIUS * 2.0 + 2.0), Sense::click());
    paint_dot(ui.painter(), dot_rect.center(), workspace.row.dot);
    // A compact row has no status line, so its dot tells it.
    let meaning = dot_meaning(workspace.row.dot);
    let spoken = if compact && !workspace.row.line.is_empty() {
        format!("{meaning}\n{}", workspace.row.line)
    } else {
        meaning.to_owned()
    };
    dot_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &spoken));
    let dot_response = dot_response.on_hover_text(spoken);
    track(&dot_response);
    ui.add_space(8.0);

    let name = egui::RichText::new(&workspace.name)
        .color(if workspace.is_active {
            theme::FG()
        } else {
            theme::FG_SOFT()
        })
        .size(if compact { 12.5 } else { 13.0 })
        .strong();
    let name_width = sidebar_workspace_name_width(ui.available_width(), workspace.detached, accordion);
    let text_height = if compact { 18.0 } else { 32.0 };
    let line = (!compact && !workspace.row.line.is_empty()).then_some(workspace.row.line.as_str());
    // A sized, top-down scope keeps the name and its line flush left while
    // letting both truncate.
    ui.allocate_ui_with_layout(Vec2::new(name_width, text_height), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let response = ui.add(egui::Label::new(name).truncate().sense(Sense::click()));
        track(&response);
        if let Some(line) = line {
            let response = ui
                .add(
                    egui::Label::new(egui::RichText::new(line).color(theme::FG_DIM()).size(11.0))
                        .truncate()
                        .sense(Sense::click()),
                )
                .on_hover_text(line);
            track(&response);
        }
    });
    if workspace.detached {
        ui.add_space(4.0);
        let detached_response = ui.add(
            egui::Label::new(
                egui::RichText::new("NEW WINDOW")
                    .color(theme::FG_DIM())
                    .size(8.5)
                    .strong(),
            )
            .sense(Sense::click()),
        );
        track(&detached_response);
    }

    if accordion {
        let count_response = ui.add(
            egui::Label::new(
                egui::RichText::new(workspace.panels.len().to_string())
                    .color(theme::FG_DIM())
                    .size(11.0),
            )
            .sense(Sense::click()),
        );
        track(&count_response);
    }

    SidebarWorkspaceRowInteraction { hovered, clicked }
}

pub(super) fn paint_workspace_row_bg(
    ui: &mut egui::Ui,
    workspace_rect: Rect,
    workspace_color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let workspace_bg = Rect::from_min_max(
        Pos2::new(workspace_rect.min.x + 6.0, workspace_rect.min.y),
        Pos2::new(workspace_rect.max.x - 6.0, workspace_rect.max.y),
    );
    if dragging {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.18), 180),
        );
    } else if is_active {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.12), 140),
        );
    } else if hovered {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::PANEL_BG_ALT(), 160),
        );
    }
}

pub(super) fn paint_workspace_drop_indicator(
    ui: &egui::Ui,
    workspace_rect: Rect,
    insert: SidebarWorkspaceInsert,
    workspace_color: Color32,
) {
    let y = match insert {
        SidebarWorkspaceInsert::Before => workspace_rect.min.y + 1.0,
        SidebarWorkspaceInsert::After => workspace_rect.max.y - 1.0,
    };
    let left = workspace_rect.min.x + 12.0;
    let right = workspace_rect.max.x - 12.0;
    ui.painter().line_segment(
        [Pos2::new(left, y), Pos2::new(right, y)],
        Stroke::new(2.0_f32, theme::alpha(workspace_color, 220)),
    );
}

pub(super) fn paint_panel_row_bg(
    ui: &mut egui::Ui,
    item_rect: Rect,
    workspace_color: Color32,
    is_focused: bool,
    hovered: bool,
) {
    let bg_rect = Rect::from_min_max(
        Pos2::new(item_rect.min.x + 6.0, item_rect.min.y),
        Pos2::new(item_rect.max.x - 6.0, item_rect.max.y),
    );
    if is_focused {
        ui.painter_at(bg_rect).rect_filled(
            bg_rect,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.22), 200),
        );
        let edge = Rect::from_min_size(
            Pos2::new(bg_rect.min.x, bg_rect.min.y + 4.0),
            Vec2::new(2.0, bg_rect.height() - 8.0),
        );
        ui.painter().rect_filled(edge, CornerRadius::same(1), workspace_color);
    } else if hovered {
        ui.painter_at(bg_rect).rect_filled(
            bg_rect,
            CornerRadius::same(10),
            theme::alpha(theme::PANEL_BG_ALT(), 180),
        );
    }
}
