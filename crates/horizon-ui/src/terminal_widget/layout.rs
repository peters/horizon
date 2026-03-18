use egui::{Pos2, Rect, Vec2};
use horizon_core::TerminalSide;

use crate::input;

pub(super) const SCROLLBAR_WIDTH: f32 = 12.0;
pub(super) const SCROLLBAR_GAP: f32 = 8.0;

pub(super) struct GridMetrics {
    pub(super) char_width: f32,
    pub(super) line_height: f32,
    pub(super) font_id: egui::FontId,
}

#[derive(Clone, Copy)]
pub(crate) struct TerminalViewportSize {
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) cell_width: u16,
    pub(crate) cell_height: u16,
}

#[derive(Clone, Copy)]
pub(super) struct TerminalLayout {
    pub(super) outer: Rect,
    pub(super) body: Rect,
    pub(super) scrollbar: Rect,
}

pub(super) struct TerminalInteraction {
    pub(super) layout: TerminalLayout,
    pub(super) body: egui::Response,
    pub(super) scrollbar: egui::Response,
}

pub(super) fn terminal_layout(available: Vec2, char_width: f32, line_height: f32) -> TerminalLayout {
    let body_size = Vec2::new(
        (available.x - SCROLLBAR_WIDTH - SCROLLBAR_GAP).max(char_width),
        available.y.max(line_height),
    );
    let outer = Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(body_size.x + SCROLLBAR_WIDTH + SCROLLBAR_GAP, body_size.y),
    );
    let body = Rect::from_min_size(outer.min, body_size);
    let scrollbar = Rect::from_min_size(
        Pos2::new(body.max.x + SCROLLBAR_GAP, outer.min.y + 4.0),
        Vec2::new(SCROLLBAR_WIDTH, (outer.height() - 8.0).max(24.0)),
    );

    TerminalLayout { outer, body, scrollbar }
}

pub(crate) fn terminal_viewport_size(available: Vec2, char_width: f32, line_height: f32) -> TerminalViewportSize {
    let layout = terminal_layout(available, char_width, line_height);

    TerminalViewportSize {
        rows: quantize_dimension(layout.body.height() / line_height).max(1),
        cols: quantize_dimension(layout.body.width() / char_width).max(2),
        cell_width: quantize_dimension(char_width),
        cell_height: quantize_dimension(line_height),
    }
}

pub(super) fn terminal_interaction(ui: &mut egui::Ui, layout: TerminalLayout, panel_id: u64) -> TerminalInteraction {
    let (allocated_rect, _) = ui.allocate_exact_size(layout.outer.size(), egui::Sense::hover());
    let layout = TerminalLayout {
        outer: allocated_rect,
        body: layout.body.translate(allocated_rect.min.to_vec2()),
        scrollbar: layout.scrollbar.translate(allocated_rect.min.to_vec2()),
    };
    let body = ui.interact(
        layout.body,
        ui.make_persistent_id(("terminal_body", panel_id)),
        egui::Sense::click_and_drag(),
    );
    let scrollbar = ui.interact(
        layout.scrollbar.expand2(Vec2::new(2.0, 2.0)),
        ui.make_persistent_id(("terminal_scrollbar", panel_id)),
        egui::Sense::click_and_drag(),
    );

    TerminalInteraction {
        layout,
        body,
        scrollbar,
    }
}

pub(super) fn cell_side(pos: Pos2, body_rect: Rect, metrics: &GridMetrics, point: input::GridPoint) -> TerminalSide {
    let cell_x = body_rect.min.x + usize_to_f32(point.column) * metrics.char_width;
    let mid = cell_x + metrics.char_width / 2.0;
    if pos.x < mid {
        TerminalSide::Left
    } else {
        TerminalSide::Right
    }
}

pub(super) fn grid_point_from_position(
    rect: Rect,
    position: Pos2,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) -> Option<input::GridPoint> {
    if !rect.contains(position) {
        return None;
    }

    let relative = position - rect.min;
    let row = (relative.y / metrics.line_height).floor().max(0.0);
    let column = (relative.x / metrics.char_width).floor().max(0.0);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        Some(input::GridPoint {
            line: (row as usize).min(usize::from(visible_rows.saturating_sub(1))),
            column: (column as usize).min(usize::from(visible_cols.saturating_sub(1))),
        })
    }
}

pub(super) fn quantize_dimension(value: f32) -> u16 {
    let clamped = if value.is_finite() {
        value.floor().clamp(1.0, f32::from(u16::MAX))
    } else {
        1.0
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        clamped as u16
    }
}

pub(super) fn quantize_visible_rows(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(super) fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

pub(super) fn f32_to_usize(value: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as usize
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GridMetrics, SCROLLBAR_GAP, SCROLLBAR_WIDTH, grid_point_from_position, quantize_dimension, terminal_layout,
    };
    use egui::{FontId, Pos2, Rect, Vec2};

    fn metrics() -> GridMetrics {
        GridMetrics {
            char_width: 8.0,
            line_height: 16.0,
            font_id: FontId::monospace(13.0),
        }
    }

    #[test]
    fn terminal_layout_reserves_space_for_scrollbar() {
        let layout = terminal_layout(Vec2::new(400.0, 240.0), 8.0, 16.0);

        assert!((layout.body.width() - (400.0 - SCROLLBAR_WIDTH - SCROLLBAR_GAP)).abs() <= f32::EPSILON);
        assert!((layout.outer.width() - 400.0).abs() <= f32::EPSILON);
        assert!(layout.scrollbar.left() >= layout.body.right() + SCROLLBAR_GAP);
    }

    #[test]
    fn grid_points_are_clamped_to_visible_terminal_bounds() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(80.0, 48.0));
        let point = grid_point_from_position(rect, Pos2::new(179.0, 127.0), &metrics(), 3, 10)
            .expect("point inside terminal grid");

        assert_eq!(point.line, 2);
        assert_eq!(point.column, 9);
    }

    #[test]
    fn quantize_dimension_handles_non_finite_values() {
        assert_eq!(quantize_dimension(f32::NAN), 1);
        assert_eq!(quantize_dimension(f32::INFINITY), 1);
        assert_eq!(quantize_dimension(f32::from(u16::MAX) + 100.0), u16::MAX);
        assert_eq!(quantize_dimension(7.9), 7);
    }
}
