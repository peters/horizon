//! The drawings of the tiles, on a window that changes size.
//!
//! The drawing of a tile is a band and the shapes on it. This test walks the page
//! through the sizes of a window that a person resizes and reads every drawing of
//! the frame back from the shapes that the page painted, so a band that the page
//! drew for an earlier size shows here as a drawing outside its own tile.

use egui::Context;

use super::tiles_app;
use super::{frame_shapes_at, painted_drawings, tile_rects};

/// The sizes of the live look that found the stale drawings, and the first size
/// again, so a page that comes back to a size paints it as it did.
const SIZES: [[f32; 2]; 6] = [
    [1600.0, 1000.0],
    [2560.0, 1300.0],
    [1280.0, 800.0],
    [1000.0, 700.0],
    [800.0, 600.0],
    [1600.0, 1000.0],
];

#[test]
fn the_drawing_of_a_tile_stands_in_the_band_of_its_tile_after_a_resize() {
    let (_temp, mut app) = tiles_app();
    let ctx = Context::default();

    for size in SIZES {
        let shapes = frame_shapes_at(&ctx, &mut app, size);
        let tiles = tile_rects(&shapes);
        let drawings = painted_drawings(&shapes);
        // The page of a window that holds no band draws the four tiles without
        // their drawings; a page that draws one drawing draws four.
        assert!(
            drawings.is_empty() || drawings.len() == tiles.len(),
            "the page draws one drawing per tile at {size:?}, or none: {} drawings, {} tiles",
            drawings.len(),
            tiles.len()
        );
        for (tile, drawing) in tiles.iter().zip(drawings.iter()) {
            let inner = tile.shrink(super::TILE_PAD);
            assert!(
                (drawing.band.min.x - inner.min.x).abs() < 0.6,
                "the band of a drawing starts at the left edge of its tile at {size:?}: {} against {}",
                drawing.band.min.x,
                inner.min.x
            );
            assert!(
                (drawing.band.min.y - inner.min.y).abs() < 0.6,
                "the band of a drawing stands under the top edge of its tile at {size:?}: {} against {}",
                drawing.band.min.y,
                inner.min.y
            );
            assert!(
                (drawing.band.width() - inner.width()).abs() < 0.6,
                "the band of a drawing is as wide as its tile at {size:?}: {} against {}",
                drawing.band.width(),
                inner.width()
            );
            assert!(
                tile.expand(0.6).contains_rect(drawing.bounds),
                "every shape of a drawing stays inside its tile at {size:?}: {:?} outside {tile:?}",
                drawing.bounds
            );
            assert!(
                drawing.band.expand(0.6).contains_rect(drawing.bounds),
                "every shape of a drawing stays inside its band at {size:?}: {:?} outside {:?}",
                drawing.bounds,
                drawing.band
            );
        }
    }
}
