//! The page of a small window: it first becomes compact, and only a page that is
//! still taller than the canvas scrolls.
//!
//! Every assertion here reads the shapes of a frame of the window, so a person
//! sees what the test sees.

use egui::{Context, Key};

use super::tiles_app;
use super::{frame_shapes_at, frame_with, key_press, text_box, tile_rects};
use crate::app::welcome::{HEADING, SHORTCUTS_LEAD, WelcomeTile};

/// The two smallest windows of the live look, which hold no whole page.
const SMALL: [[f32; 2]; 2] = [[1000.0, 700.0], [800.0, 600.0]];

/// Presses one key and answers the shapes of that frame.
fn press(ctx: &Context, app: &mut crate::app::HorizonApp, size: [f32; 2], key: Key) -> Vec<egui::epaint::ClippedShape> {
    let _ = frame_with(ctx, app, size, vec![key_press(key, true)]);
    frame_with(ctx, app, size, vec![key_press(key, false)])
}

/// Walks the page to its top, then one page at a time until the tile at `index`
/// stands whole in the canvas, and answers whether it did.
fn scroll_to_tile(ctx: &Context, app: &mut crate::app::HorizonApp, size: [f32; 2], index: usize) -> bool {
    for _ in 0..24 {
        let _ = press(ctx, app, size, Key::PageUp);
    }
    let canvas = app.canvas_rect(ctx);
    for _ in 0..24 {
        let shapes = press(ctx, app, size, Key::PageDown);
        if let Some(rect) = tile_rects(&shapes).get(index)
            && canvas.expand(0.6).contains_rect(*rect)
        {
            return true;
        }
    }
    false
}

#[test]
fn a_small_window_reaches_the_four_choices_and_the_last_row() {
    for size in SMALL {
        let (_temp, mut app) = tiles_app();
        let ctx = Context::default();
        let shapes = frame_shapes_at(&ctx, &mut app, size);
        let canvas = app.canvas_rect(&ctx);

        let heading = text_box(&shapes, HEADING).expect("the heading is drawn");
        assert!(
            canvas.contains_rect(heading),
            "the heading is whole at the top of a window of {size:?}: {heading:?} outside {canvas:?}"
        );

        for (index, tile) in WelcomeTile::ALL.iter().enumerate() {
            assert!(
                scroll_to_tile(&ctx, &mut app, size, index),
                "the tile {:?} can be brought whole into the canvas of a window of {size:?}",
                tile.title()
            );
        }

        let mut shapes = Vec::new();
        for _ in 0..24 {
            shapes = press(&ctx, &mut app, size, Key::PageDown);
        }
        let last = text_box(&shapes, SHORTCUTS_LEAD).expect("the row of shortcuts is drawn");
        assert!(
            canvas.expand(0.6).contains_rect(last),
            "the last row of the page is reachable at the largest offset of a window of {size:?}: {last:?} \
             outside {canvas:?}"
        );
    }
}

#[test]
fn the_wheel_moves_a_page_that_is_taller_than_the_canvas() {
    let size = [800.0, 600.0];
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, size);
    let top = tile_rects(&shapes)[0].min.y;
    let page = app.canvas_rect(&ctx).center();

    let _ = frame_with(&ctx, &mut app, size, vec![egui::Event::PointerMoved(page)]);
    let _ = frame_with(
        &ctx,
        &mut app,
        size,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let after = frame_with(&ctx, &mut app, size, Vec::new());
    let moved = tile_rects(&after)[0].min.y;
    assert!(
        moved < top - 1.0,
        "the wheel moves the page up: the first tile stands at {moved}, and it stood at {top}"
    );
}

#[test]
fn tab_brings_a_tile_below_the_canvas_into_view() {
    let size = [800.0, 600.0];
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let _ = frame_shapes_at(&ctx, &mut app, size);
    for _ in 0..WelcomeTile::ALL.len() {
        let _ = press(&ctx, &mut app, size, Key::Tab);
    }
    let shapes = frame_with(&ctx, &mut app, size, Vec::new());
    let canvas = app.canvas_rect(&ctx);
    let fourth = tile_rects(&shapes)[3];
    assert!(
        canvas.expand(0.6).contains_rect(fourth),
        "Tab brings the fourth tile into the canvas of a window of {size:?}: {fourth:?} outside {canvas:?}"
    );
}

#[test]
fn a_window_that_holds_the_page_keeps_it_where_it_is() {
    let size = [1600.0, 1000.0];
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, size);
    let tiles = tile_rects(&shapes);

    // The four tiles of the page before this round, read from its frames.
    let expected = [
        [270.0, 421.8, 575.5, 677.5],
        [591.5, 421.8, 897.0, 677.5],
        [913.0, 421.8, 1218.5, 677.5],
        [1234.5, 421.8, 1540.0, 677.5],
    ];
    assert_eq!(tiles.len(), expected.len());
    for (rect, expected) in tiles.iter().zip(expected) {
        let painted = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
        for (painted, expected) in painted.iter().zip(expected) {
            assert!(
                (painted - expected).abs() < 0.6,
                "the page of a window that holds it stands where it stood: {painted} against {expected}"
            );
        }
    }

    let after = press(&ctx, &mut app, size, Key::PageDown);
    assert_eq!(
        tile_rects(&after),
        tiles,
        "a page that fits the canvas of a window of {size:?} does not scroll"
    );
}
