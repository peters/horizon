//! The workspace rows of the sidebar's cloud list: a tile in the workspace color
//! with its initials and a status badge, the name and one status line, or only
//! the tile and the name for a compact parked row.
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use horizon_core::cloud_list::{Dot, Group};

use crate::theme;

use super::text::{self, BOLD, SEMIBOLD};
use super::{SidebarWorkspaceInsert, WorkspaceSidebarEntry};

/// The height of a row with a status line.
pub(super) const ROW_HEIGHT: f32 = 50.0;
/// The height of a compact parked row.
pub(super) const COMPACT_ROW_HEIGHT: f32 = 38.0;
/// The horizontal inset of a row's card inside the sidebar.
pub(super) const ROW_INSET: f32 = 8.0;
/// The left edge of the tile inside a row's card.
pub(super) const TILE_INSET: f32 = 8.0;
/// The side of the tile of a row with a status line.
pub(super) const TILE_SIDE: f32 = 30.0;
const COMPACT_TILE_SIDE: f32 = 26.0;
const DOT_RADIUS: f32 = 4.0;
const CARD_RADIUS: u8 = 12;

/// Whether the row of `workspace` is compact: a parked row shows no status line.
pub(super) fn is_compact(workspace: &WorkspaceSidebarEntry) -> bool {
    workspace.row.group == Group::Parked
}

pub(super) fn row_height(workspace: &WorkspaceSidebarEntry) -> f32 {
    if is_compact(workspace) {
        COMPACT_ROW_HEIGHT
    } else {
        ROW_HEIGHT
    }
}

/// The card of a row: the row without the sidebar inset.
pub(super) fn card(row: Rect) -> Rect {
    row.shrink2(vec2(ROW_INSET, 0.0))
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

/// Whether the tile shows a badge: an idle row shows none, so activity stands out.
const fn has_badge(dot: Dot) -> bool {
    !matches!(dot, Dot::Idle)
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

/// Paints the card of a row: a filled, outlined card for the active workspace,
/// a lighter one while dragged or hovered.
pub(super) fn paint_workspace_row_bg(
    ui: &egui::Ui,
    row: Rect,
    workspace_color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let card = card(row);
    let painter = ui.painter();
    if dragging {
        painter.rect_filled(
            card,
            CARD_RADIUS,
            theme::blend(theme::PANEL_BG_ALT(), workspace_color, 0.22),
        );
    } else if is_active {
        painter.rect_filled(card, CARD_RADIUS, theme::PANEL_BG_ALT());
        painter.rect_stroke(
            card,
            CARD_RADIUS,
            Stroke::new(1.0_f32, theme::alpha(workspace_color, 120)),
            StrokeKind::Inside,
        );
    } else if hovered {
        painter.rect_filled(card, CARD_RADIUS, theme::alpha(theme::PANEL_BG_ALT(), 170));
    }
}

/// Paints the tile, the name with its badges and, unless the row is compact, the
/// status line. The tile tells its status to a pointer and a screen reader.
pub(super) fn paint_workspace_row_contents(
    ui: &egui::Ui,
    row: Rect,
    workspace: &WorkspaceSidebarEntry,
    accordion: bool,
) -> egui::Response {
    let card = card(row);
    let compact = is_compact(workspace);
    let line = (!compact && !workspace.row.line.is_empty()).then(|| text::fold_whitespace(&workspace.row.line));
    let first_line_y = if line.is_some() {
        card.min.y + 17.0
    } else {
        card.center().y
    };

    let tile = paint_tile(ui, card, workspace, compact);
    let text_left = tile.max.x + 10.0;
    let (name_right, badge_shows) = paint_badges(ui, card, workspace, accordion, text_left, first_line_y);
    let meaning = dot_meaning(workspace.row.dot);
    let mut spoken = if compact && !workspace.row.line.is_empty() {
        format!("{meaning}\n{}", workspace.row.line)
    } else {
        meaning.to_owned()
    };
    // A narrow sidebar leaves out the badge, so the tile tells it.
    if workspace.detached && !badge_shows {
        spoken.push_str("\nIn a new window");
    }
    let tile_response = ui.interact(
        tile.expand(2.0),
        ui.make_persistent_id(("sidebar_ws_tile", workspace.id.0)),
        Sense::hover(),
    );
    tile_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &spoken));
    let tile_response = tile_response.on_hover_text(spoken);

    let name_color = if workspace.is_active || has_badge(workspace.row.dot) {
        theme::FG()
    } else {
        theme::blend(theme::FG(), theme::FG_SOFT(), 0.45)
    };
    let name = text::line(ui, &workspace.name, 13.5, SEMIBOLD, name_color, name_right - text_left);
    let name_height = name.size().y;
    let name_rect = Rect::from_min_size(pos2(text_left, first_line_y - name_height / 2.0), name.size());
    let name_elided = name.elided;
    ui.painter().galley(name_rect.min, name, name_color);
    if name_elided {
        hover_text(ui, name_rect, ("sidebar_ws_name", workspace.id.0), &workspace.name);
    }
    if let Some(line) = line {
        let galley = text::line(
            ui,
            &line,
            12.0,
            text::REGULAR,
            text::secondary(),
            card.max.x - 10.0 - text_left,
        );
        let line_rect = Rect::from_min_size(pos2(text_left, name_rect.max.y + 2.0), galley.size());
        ui.painter().galley(line_rect.min, galley, text::secondary());
        hover_text(ui, line_rect, ("sidebar_ws_line", workspace.id.0), &line);
    }
    tile_response
}

/// Shows `text` while the pointer is on `rect`, and tells it to a screen reader.
fn hover_text(ui: &egui::Ui, rect: Rect, id: (&str, u64), text: &str) {
    let response = ui.interact(rect, ui.make_persistent_id(id), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    response.on_hover_text(text);
}

/// Paints the tile with its initials and status badge; returns its rect.
fn paint_tile(ui: &egui::Ui, card: Rect, workspace: &WorkspaceSidebarEntry, compact: bool) -> Rect {
    let painter = ui.painter();
    let accent = workspace.color;
    let side = if compact { COMPACT_TILE_SIDE } else { TILE_SIDE };
    let tile = Rect::from_min_size(
        pos2(card.min.x + TILE_INSET, card.center().y - side / 2.0),
        Vec2::splat(side),
    );
    let fill = theme::blend(
        theme::PANEL_BG_ALT(),
        accent,
        if workspace.is_active { 0.42 } else { 0.26 },
    );
    painter.rect_filled(tile, 9, fill);
    let letters_color = theme::ensure_terminal_text_contrast(theme::blend(accent, theme::FG(), 0.35), fill);
    let letters = text::line(
        ui,
        &text::initials(&workspace.name),
        if compact { 11.0 } else { 12.0 },
        BOLD,
        letters_color,
        side,
    );
    painter.galley(tile.center() - letters.size() / 2.0, letters, letters_color);

    if has_badge(workspace.row.dot) {
        let center = tile.right_bottom() - vec2(2.0, 2.0);
        let ring = if workspace.is_active {
            theme::PANEL_BG_ALT()
        } else {
            theme::BG_ELEVATED()
        };
        painter.circle_filled(center, DOT_RADIUS + 2.0, ring);
        paint_dot(painter, center, workspace.row.dot);
    }
    tile
}

/// The least room that the name keeps before the `NEW WINDOW` badge gives way.
const MIN_NAME_WIDTH: f32 = 48.0;
const COUNT_GAP: f32 = 8.0;
const BADGE_GAP: f32 = 6.0;

/// Where the name must end, from the right edge of the first line: after the
/// panel count of `count_width` and the `NEW WINDOW` badge of `badge_width`.
/// The badge shows only when the name keeps `MIN_NAME_WIDTH`; the second value
/// tells whether it shows.
pub(super) fn name_right(
    text_left: f32,
    right: f32,
    count_width: Option<f32>,
    badge_width: Option<f32>,
) -> (f32, bool) {
    let right = count_width.map_or(right, |width| right - width - COUNT_GAP);
    match badge_width {
        Some(width) if right - width - BADGE_GAP - text_left >= MIN_NAME_WIDTH => (right - width - BADGE_GAP, true),
        _ => (right, false),
    }
}

/// Paints the panel count (accordion only) and, when it fits, the `NEW WINDOW`
/// badge at the right of the first line. Returns where the name must end and
/// whether the badge shows.
fn paint_badges(
    ui: &egui::Ui,
    card: Rect,
    workspace: &WorkspaceSidebarEntry,
    accordion: bool,
    text_left: f32,
    y: f32,
) -> (f32, bool) {
    let right = card.max.x - 10.0;
    let count_color = if workspace.is_active {
        theme::FG_SOFT()
    } else {
        theme::FG_DIM()
    };
    let count = accordion.then(|| text::mono(ui, &workspace.panels.len().to_string(), 11.0, count_color));
    let badge = workspace
        .detached
        .then(|| text::line(ui, "NEW WINDOW", 8.5, SEMIBOLD, theme::FG_DIM(), 80.0));
    let (name_right, badge_shows) = name_right(
        text_left,
        right,
        count.as_ref().map(|count| count.size().x),
        badge.as_ref().map(|badge| badge.size().x),
    );
    if let Some(count) = count {
        let pos = pos2(right - count.size().x, y - count.size().y / 2.0);
        ui.painter().galley(pos, count, count_color);
    }
    if let Some(badge) = badge.filter(|_| badge_shows) {
        let pos = pos2(name_right + BADGE_GAP, y - badge.size().y / 2.0);
        ui.painter().galley(pos, badge, theme::FG_DIM());
    }
    (name_right, badge_shows)
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
