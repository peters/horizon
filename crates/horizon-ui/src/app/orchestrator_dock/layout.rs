//! Where the orchestrator dock stands: its width, its rectangle in this frame,
//! and whether it leaves the canvas enough room.
//!
//! Every number here is a rule of the layout, so the frame of the dock and the
//! canvas read one rectangle. The dock keeps the right edge of the window, so an
//! open Settings panel stands to its left and the canvas ends at the left edge
//! of the leftmost of the two.

use egui::{Pos2, Rect, vec2};

use crate::app::TOOLBAR_HEIGHT;

/// The width of the dock is this part of the window, between the two bounds.
pub(super) const DOCK_WIDTH_FRACTION: f32 = 0.27;
pub(super) const DOCK_MIN_WIDTH: f32 = 360.0;
pub(super) const DOCK_MAX_WIDTH: f32 = 540.0;
/// The least width that the canvas keeps beside the dock and the Settings panel:
/// the least side of a panel.
pub(super) const MIN_CANVAS_WIDTH: f32 = crate::app::PANEL_MIN_SIZE[0];

/// The width of the dock in a window of `viewport_width` points.
pub(super) fn dock_width(viewport_width: f32) -> f32 {
    (viewport_width * DOCK_WIDTH_FRACTION).clamp(DOCK_MIN_WIDTH, DOCK_MAX_WIDTH)
}

/// Whether the dock of `width` leaves the canvas [`MIN_CANVAS_WIDTH`] points
/// beside the sidebar and the Settings panel of `settings_width`.
pub(super) fn dock_fits(viewport_width: f32, sidebar_width: f32, settings_width: f32, width: f32) -> bool {
    viewport_width - sidebar_width - settings_width - width >= MIN_CANVAS_WIDTH
}

/// The rectangle of the dock in `viewport`: the right edge of the window, from
/// under the toolbar to its bottom.
pub(super) fn dock_rect(viewport: Rect, width: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(viewport.max.x - width, viewport.min.y + TOOLBAR_HEIGHT),
        viewport.max,
    )
}

/// The room of the dock inside its rectangle, from the top down: the header, the
/// body under it, and the composer at the bottom edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Parts {
    pub(super) header: Rect,
    pub(super) body: Rect,
    pub(super) composer: Rect,
}

/// Splits `content` into the header of `header_height` points at its top, the
/// composer of `composer_height` points at its bottom edge, and the body between
/// them. A composer that needs more than the room keeps the room and leaves the
/// body empty, so no part of the dock leaves the dock.
pub(super) fn parts(content: Rect, header_height: f32, composer_height: f32) -> Parts {
    let header_height = header_height.clamp(0.0, content.height());
    let composer_height = composer_height.clamp(0.0, content.height() - header_height);
    let header = Rect::from_min_size(content.min, vec2(content.width(), header_height));
    let composer = Rect::from_min_max(Pos2::new(content.min.x, content.max.y - composer_height), content.max);
    let body = Rect::from_min_max(
        Pos2::new(content.min.x, header.max.y),
        Pos2::new(content.max.x, composer.min.y),
    );
    Parts { header, body, composer }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::{DOCK_MAX_WIDTH, DOCK_MIN_WIDTH, dock_fits, dock_rect, dock_width, parts};
    use crate::app::TOOLBAR_HEIGHT;

    fn nearly(left: f32, right: f32) -> bool {
        (left - right).abs() < 0.01
    }

    #[test]
    fn the_dock_takes_a_part_of_the_window_between_its_two_bounds() {
        // 27 percent of the window, and the bounds where they win.
        for (viewport_width, width) in [
            (800.0, DOCK_MIN_WIDTH),
            (1600.0, 432.0),
            (2560.0, DOCK_MAX_WIDTH),
            (5120.0, DOCK_MAX_WIDTH),
        ] {
            assert!(
                nearly(dock_width(viewport_width), width),
                "the dock of a window of {viewport_width} points is {} wide, not {width}",
                dock_width(viewport_width)
            );
        }
    }

    #[test]
    fn the_dock_leaves_the_canvas_its_least_width() {
        // A window of 1280 points with the sidebar and the dock keeps 1280 - 210 -
        // 346 = 724 points of canvas, and the same window without the sidebar 934.
        assert!(dock_fits(1280.0, 210.0, 0.0, dock_width(1280.0)));
        // A window of 800 points with the sidebar and the dock keeps 230 points,
        // which is less than a panel.
        assert!(!dock_fits(800.0, 210.0, 0.0, DOCK_MIN_WIDTH));
        // The same window without the sidebar keeps 440 points.
        assert!(dock_fits(800.0, 0.0, 0.0, DOCK_MIN_WIDTH));
        // A Settings panel of the least width takes the room of the canvas of a
        // window of 800 points, and leaves it in a window of 1600.
        assert!(!dock_fits(800.0, 210.0, 444.0, DOCK_MIN_WIDTH));
        assert!(dock_fits(1600.0, 210.0, 480.0, dock_width(1600.0)));
        assert!(!dock_fits(1600.0, 210.0, 700.0, dock_width(1600.0)));
    }

    #[test]
    fn the_dock_hangs_on_the_right_edge_under_the_toolbar() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1600.0, 1000.0));

        assert_eq!(
            dock_rect(viewport, 432.0),
            Rect::from_min_max(Pos2::new(1168.0, TOOLBAR_HEIGHT), Pos2::new(1600.0, 1000.0))
        );
    }

    #[test]
    fn the_header_keeps_the_top_and_the_composer_the_bottom_edge() {
        let content = Rect::from_min_max(Pos2::new(1168.0, 46.0), Pos2::new(1600.0, 1000.0));

        let split = parts(content, 47.0, 194.0);

        // The header takes the top of the room, the composer the last 194 points,
        // and the body is all the room between them.
        assert_eq!(split.header, Rect::from_min_max(content.min, Pos2::new(1600.0, 93.0)));
        assert_eq!(
            split.composer,
            Rect::from_min_max(Pos2::new(1168.0, 806.0), content.max)
        );
        assert_eq!(
            split.body,
            Rect::from_min_max(Pos2::new(1168.0, 93.0), Pos2::new(1600.0, 806.0))
        );
        assert_eq!(split.body.height(), content.height() - 47.0 - 194.0);
    }

    #[test]
    fn a_composer_that_needs_the_whole_room_leaves_the_body_empty() {
        let content = Rect::from_min_max(Pos2::new(100.0, 50.0), Pos2::new(500.0, 650.0));

        let split = parts(content, 47.0, 1000.0);

        // No part leaves the room of the dock, and the composer keeps the bottom
        // edge under the header.
        assert_eq!(split.header.min, content.min);
        assert_eq!(split.composer.max, content.max);
        assert_eq!(split.composer.min.y, 97.0);
        assert_eq!(split.body.height(), 0.0);
        assert!(content.contains_rect(split.header));
        assert!(content.contains_rect(split.composer));
    }
}
