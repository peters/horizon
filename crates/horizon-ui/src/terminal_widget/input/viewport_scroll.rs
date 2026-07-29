use std::time::Duration;

use egui::{Pos2, Rect};
use horizon_core::{Panel, SelectionType, TerminalSide};

use super::super::layout::{GridMetrics, cell_side, grid_point_from_position};
use super::PointerContext;

pub(super) const SELECTION_AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(16);

pub(super) fn handle_pointer_selection_drag(
    panel: &mut Panel,
    pos: Pos2,
    body_rect: Rect,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) -> bool {
    if pos.y < body_rect.min.y {
        let scrollback_before = panel.terminal().map_or(0, horizon_core::Terminal::scrollback);
        let overshoot = body_rect.min.y - pos.y;
        let lines = selection_scroll_lines(overshoot, metrics.line_height);
        panel.scroll_scrollback_by(lines);
        update_pointer_selection_endpoint(panel, pos, body_rect, metrics, visible_rows, visible_cols);
        panel.terminal().map_or(0, horizon_core::Terminal::scrollback) != scrollback_before
    } else if pos.y > body_rect.max.y {
        let scrollback_before = panel.terminal().map_or(0, horizon_core::Terminal::scrollback);
        let overshoot = pos.y - body_rect.max.y;
        let lines = selection_scroll_lines(overshoot, metrics.line_height);
        panel.scroll_scrollback_by(-lines);
        update_pointer_selection_endpoint(panel, pos, body_rect, metrics, visible_rows, visible_cols);
        panel.terminal().map_or(0, horizon_core::Terminal::scrollback) != scrollback_before
    } else {
        update_pointer_selection_endpoint(panel, pos, body_rect, metrics, visible_rows, visible_cols);
        false
    }
}

pub(super) fn start_local_selection_at(panel: &mut Panel, pointer: &PointerContext<'_>, pos: Pos2) {
    let body_rect = pointer.interaction.layout.body;
    if let Some(point) = grid_point_from_position(
        body_rect,
        pos,
        pointer.metrics,
        pointer.visible_rows,
        pointer.visible_cols,
    ) {
        let sel_type = if pointer.interaction.body.triple_clicked() {
            SelectionType::Lines
        } else if pointer.interaction.body.double_clicked() {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        let side = cell_side(pos, body_rect, pointer.metrics, point);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.start_selection(sel_type, point.line, point.column, side);
        }
    }
}

pub(super) fn update_pointer_selection_endpoint(
    panel: &mut Panel,
    pos: Pos2,
    body_rect: Rect,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) {
    if pos.y < body_rect.min.y {
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(0, 0, TerminalSide::Left);
        }
    } else if pos.y > body_rect.max.y {
        let last_row = visible_rows.saturating_sub(1);
        let last_col = visible_cols.saturating_sub(1);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(usize::from(last_row), usize::from(last_col), TerminalSide::Right);
        }
    } else if let Some(point) = grid_point_from_position(body_rect, pos, metrics, visible_rows, visible_cols) {
        let side = cell_side(pos, body_rect, metrics, point);
        if let Some(terminal) = panel.terminal_mut() {
            terminal.update_selection(point.line, point.column, side);
        }
    }
}

fn selection_scroll_lines(overshoot: f32, line_height: f32) -> i32 {
    let lines = (overshoot / line_height).ceil().max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (lines as i32).min(5)
    }
}
