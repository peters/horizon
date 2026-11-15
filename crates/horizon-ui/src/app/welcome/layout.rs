//! The geometry of the first start page: how many tiles stand beside each
//! other, and how wide they are.
//!
//! The page answers the width of the canvas: four tiles in one row on a wide
//! window, two rows of two under it, and one column on a narrow one.

/// The space between two tiles of a row, in whole points.
pub(in crate::app) const TILE_GAP: f32 = 16.0;
/// The space between two rows of tiles, in whole points.
pub(in crate::app) const ROW_GAP: f32 = 16.0;
/// The widest that the block of the page grows, in whole points. A canvas that is
/// wider leaves the block in the middle with the same air on both sides, so a
/// heading, a sentence and a row of the section never run the whole width of a
/// large screen.
pub(in crate::app) const MAX_BLOCK_WIDTH: f32 = 1480.0;
/// The smallest canvas width that holds four tiles in one row.
pub(in crate::app) const FOUR_COLUMNS_MIN_WIDTH: f32 = 900.0;
/// The smallest canvas width that holds two tiles in a row.
pub(in crate::app) const TWO_COLUMNS_MIN_WIDTH: f32 = 520.0;

/// The width of the block of the page on a canvas `canvas_width` wide: the canvas
/// without the pads of the page, and at most the reading width of the page, so a
/// large screen keeps the block in the middle instead of stretching every line of
/// it.
pub(in crate::app) fn page_width(canvas_width: f32) -> f32 {
    (canvas_width - 2.0 * f32::from(super::PAGE_PAD_SIDE)).clamp(1.0, MAX_BLOCK_WIDTH)
}

/// How many tiles stand beside each other on a canvas `width` wide.
pub(in crate::app) fn welcome_columns(width: f32) -> usize {
    if !width.is_finite() || width < TWO_COLUMNS_MIN_WIDTH {
        1
    } else if width < FOUR_COLUMNS_MIN_WIDTH {
        2
    } else {
        4
    }
}

/// The width of one tile in a grid of `columns` columns that takes `width`
/// points, with the gaps between them.
#[allow(clippy::cast_precision_loss)]
pub(in crate::app) fn tile_width(width: f32, columns: usize) -> f32 {
    let columns = columns.max(1);
    let gaps = TILE_GAP * (columns.saturating_sub(1)) as f32;
    ((width.max(0.0) - gaps) / columns as f32).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::{
        FOUR_COLUMNS_MIN_WIDTH, MAX_BLOCK_WIDTH, TILE_GAP, TWO_COLUMNS_MIN_WIDTH, page_width, tile_width,
        welcome_columns,
    };

    #[test]
    fn the_block_stops_at_the_reading_width_of_the_page() {
        assert_eq!(page_width(1200.0), 1200.0 - 120.0, "a canvas that fits gives its width");
        assert_eq!(
            page_width(5000.0),
            MAX_BLOCK_WIDTH,
            "a wide canvas keeps the reading width"
        );
        assert_eq!(page_width(40.0), 1.0, "a canvas narrower than its pads keeps a sliver");
    }

    #[test]
    fn the_page_answers_the_width_of_the_canvas() {
        assert_eq!(welcome_columns(FOUR_COLUMNS_MIN_WIDTH), 4);
        assert_eq!(welcome_columns(FOUR_COLUMNS_MIN_WIDTH - 1.0), 2);
        assert_eq!(welcome_columns(TWO_COLUMNS_MIN_WIDTH), 2);
        assert_eq!(welcome_columns(TWO_COLUMNS_MIN_WIDTH - 1.0), 1);
        assert_eq!(welcome_columns(400.0), 1, "a narrow window holds one column");
        assert_eq!(welcome_columns(f32::NAN), 1, "an unmeasured canvas holds one column");
    }

    #[test]
    fn the_tiles_and_their_gaps_fill_the_width() {
        for (width, columns) in [(1088.0, 4), (760.0, 2), (400.0, 1)] {
            let tile = tile_width(width, columns);
            #[allow(clippy::cast_precision_loss)]
            let gaps = TILE_GAP * (columns - 1) as f32;
            #[allow(clippy::cast_precision_loss)]
            let filled = tile * columns as f32 + gaps;
            assert!(
                (filled - width).abs() <= f32::EPSILON,
                "{columns} columns fill {width}: {filled}"
            );
        }
        assert_eq!(tile_width(0.0, 4), 0.0, "no width leaves no tile");
        assert_eq!(tile_width(-40.0, 2), 0.0, "a negative width leaves no tile");
    }
}
