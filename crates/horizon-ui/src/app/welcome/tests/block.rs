//! The width of the block of the page: it stops at a reading width and stands in
//! the middle of the canvas.

use egui::Context;

use super::tiles_app;
use super::{block_rect, frame_shapes_at, text_shape};
use crate::app::welcome::{HEADING, SHORTCUTS_LEAD, layout::MAX_BLOCK_WIDTH};

#[test]
fn a_wide_canvas_keeps_the_block_at_the_reading_width_in_the_middle() {
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, [5000.0, 1348.0]);
    let canvas = app.canvas_rect(&ctx);
    let block = block_rect(&shapes);

    assert!(
        block.width() <= MAX_BLOCK_WIDTH + 0.6,
        "the block of the page takes at most {MAX_BLOCK_WIDTH} points, not {}",
        block.width()
    );
    assert!(
        (block.center().x - canvas.center().x).abs() < 0.6,
        "the middle of the block is the middle of the canvas: {} against {}",
        block.center().x,
        canvas.center().x
    );
    let (heading, _) = text_shape(&shapes, HEADING).expect("the heading is drawn");
    let heading_centre = heading.pos.x + heading.galley.size().x * 0.5;
    assert!(
        (heading_centre - block.center().x).abs() < 0.6,
        "the heading stands in the middle of the block: {heading_centre} against {}",
        block.center().x
    );
    let (shortcuts, _) = text_shape(&shapes, SHORTCUTS_LEAD).expect("the row of shortcuts is drawn");
    assert!(
        (shortcuts.pos.x - block.min.x).abs() < 0.6,
        "the row of shortcuts starts at the left edge of the block: {} against {}",
        shortcuts.pos.x,
        block.min.x
    );
}

#[test]
fn a_canvas_under_the_reading_width_gives_the_block_the_width_of_the_canvas() {
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();
    let shapes = frame_shapes_at(&ctx, &mut app, [1200.0, 900.0]);
    let canvas = app.canvas_rect(&ctx);
    let block = block_rect(&shapes);
    let pad = f32::from(crate::app::welcome::PAGE_PAD_SIDE);

    assert!(
        (block.width() - (canvas.width() - 2.0 * pad)).abs() < 0.6,
        "the block takes the canvas without the pads of the page: {} against {}",
        block.width(),
        canvas.width() - 2.0 * pad
    );
    assert!(
        (block.min.x - (canvas.min.x + pad)).abs() < 0.6,
        "the block keeps the pad of the page at the left: {} against {}",
        block.min.x,
        canvas.min.x + pad
    );
}
