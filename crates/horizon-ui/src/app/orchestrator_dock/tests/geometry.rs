//! The rectangles that the dock and the canvas share, without a window: where the
//! canvas ends beside the dock, beside an open Settings panel, and beside both.

use egui::{Pos2, Rect};

use crate::app::actions::canvas_rect_for_layout;
use crate::app::orchestrator_dock::layout::{dock_rect, dock_width};
use crate::app::orchestrator_dock::leftmost_panel;
use crate::app::settings::{SETTINGS_BAR_HEIGHT, settings_panel_default_width};
use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

fn viewport(width: f32, height: f32) -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(width, height))
}

#[test]
fn the_canvas_ends_at_the_left_edge_of_the_dock() {
    let window = viewport(1600.0, 1000.0);
    let dock = dock_rect(window, dock_width(window.width()));

    let canvas = canvas_rect_for_layout(window, SIDEBAR_WIDTH, None, None, Some(dock));

    assert_eq!(canvas.min, Pos2::new(SIDEBAR_WIDTH, TOOLBAR_HEIGHT));
    assert_eq!(canvas.max, Pos2::new(dock.min.x, window.max.y));
}

#[test]
fn the_canvas_keeps_its_numbers_beside_a_settings_panel_alone() {
    let window = viewport(1200.0, 800.0);
    let settings_panel = Rect::from_min_max(
        Pos2::new(840.0, TOOLBAR_HEIGHT),
        Pos2::new(1200.0, 800.0 - SETTINGS_BAR_HEIGHT),
    );
    let settings_bar = Rect::from_min_max(Pos2::new(0.0, 800.0 - SETTINGS_BAR_HEIGHT), window.max);

    let canvas = canvas_rect_for_layout(window, SIDEBAR_WIDTH, Some(settings_panel), Some(settings_bar), None);

    assert_eq!(canvas.min, Pos2::new(SIDEBAR_WIDTH, TOOLBAR_HEIGHT));
    assert_eq!(canvas.max, Pos2::new(840.0, 800.0 - SETTINGS_BAR_HEIGHT));
}

#[test]
fn the_canvas_ends_at_the_leftmost_of_the_dock_and_the_settings_panel() {
    let window = viewport(2000.0, 1000.0);
    let dock = dock_rect(window, dock_width(window.width()));
    // The dock keeps the right edge of the window, so the Settings panel stands
    // inside it.
    let settings_panel = Rect::from_min_max(
        Pos2::new(1460.0 - settings_panel_default_width(window.width()), TOOLBAR_HEIGHT),
        Pos2::new(1460.0, window.max.y),
    );

    let with_both = canvas_rect_for_layout(window, SIDEBAR_WIDTH, Some(settings_panel), None, Some(dock));
    let with_dock = canvas_rect_for_layout(window, SIDEBAR_WIDTH, None, None, Some(dock));

    assert_eq!(with_both.max.x, settings_panel.min.x);
    assert!(with_dock.max.x > with_both.max.x);
    assert_eq!(leftmost_panel([Some(settings_panel), Some(dock)]), Some(settings_panel));
    assert_eq!(leftmost_panel([None, Some(dock)]), Some(dock));
    assert_eq!(leftmost_panel([None, None]), None);
}
