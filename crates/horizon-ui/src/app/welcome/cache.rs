//! What the first start page remembers between frames: where its parts stand and
//! the drawings of the four tiles.
//!
//! The page is measured once per width, theme and count of the rows of the section
//! **Pick up where you left off**, and the drawing of a tile is built once per
//! size of its band and theme, so a frame that changes nothing only paints the
//! shapes that the frame before it built. A window that a person resizes changes
//! the width of a band, which measures the page again and builds the drawings of
//! that width.
//!
//! A drawing is kept in the coordinates of its own band, not of the canvas: a
//! drawing follows the place that the page gives its band, and a frame that moves
//! the page, for example because the sidebar grew, moves the same shapes.

use egui::{Painter, Shape, Vec2};

use super::art;
use super::layout::{ROW_GAP, tile_width, welcome_columns};
use super::pickup;
use super::tiles::WelcomeTile;
use super::{
    ACTION_FONT, ACTION_LINE_HEIGHT, HEADING_BLOCK_GAP, HEADING_FONT, HEADING_GAP, HEADING_LINE_HEIGHT,
    HEADING_MAX_WIDTH, LEAD_FONT, SENTENCE_FONT, SENTENCE_LINE_HEIGHT, SHORTCUTS_FONT, SHORTCUTS_GAP, TILE_INNER_GAP,
    TILE_PAD, TITLE_FONT, TITLE_LINE_HEIGHT, tile_title_job,
};
use crate::theme;
use crate::typeface::Weight;

/// What the page remembers, so a still page lays nothing out again.
#[derive(Default)]
pub(in crate::app) struct WelcomeCache {
    /// The width, the theme and the rows of the section that the metrics belong
    /// to.
    key: Option<WelcomeKey>,
    metrics: Option<WelcomeMetrics>,
    drawings: Drawings,
    /// How many drawings the page built, so a test can ask whether a frame that
    /// changed nothing built one again.
    #[cfg(test)]
    builds: usize,
}

impl WelcomeCache {
    /// The metrics of the page for `width` and `pickup_rows` rows of the section,
    /// measured with `painter`.
    pub(in crate::app) fn metrics(&mut self, painter: &Painter, width: f32, pickup_rows: usize) -> WelcomeMetrics {
        let key = WelcomeKey {
            width: width.to_bits(),
            theme: theme::current_theme(),
            pickup_rows,
        };
        if self.key != Some(key) {
            let metrics = WelcomeMetrics::measure(painter, width, pickup_rows);
            self.key = Some(key);
            self.metrics = Some(metrics);
        }
        self.metrics
            .unwrap_or_else(|| WelcomeMetrics::measure(painter, width, pickup_rows))
    }

    /// The drawing of `tile`, the tile at `index`, for a band of `size`, in the
    /// coordinates of the band.
    ///
    /// The page keeps one drawing per tile: the shapes are built again only when
    /// the size of the band or the theme changes, and a frame of a page that
    /// changed nothing paints the shapes of the frame before.
    pub(in crate::app) fn drawing(&mut self, tile: WelcomeTile, index: usize, size: Vec2) -> Shape {
        let key = DrawingKey {
            width: size.x.to_bits(),
            height: size.y.to_bits(),
            theme: theme::current_theme(),
        };
        if self.drawings.key != Some(key) {
            self.drawings = Drawings {
                key: Some(key),
                shapes: Default::default(),
            };
        }
        let slot = &mut self.drawings.shapes[index];
        #[cfg(test)]
        let built = slot.is_none();
        let shape = slot.get_or_insert_with(|| Shape::Vec(art::local(tile, size))).clone();
        #[cfg(test)]
        if built {
            self.builds += 1;
        }
        shape
    }

    /// How many drawings of tiles the page built, for a test that asks whether a
    /// frame of a page that changed nothing built one again.
    #[cfg(test)]
    pub(in crate::app) fn builds(&self) -> usize {
        self.builds
    }
}

/// The drawings of the four tiles, in the coordinates of their own band.
#[derive(Default)]
struct Drawings {
    key: Option<DrawingKey>,
    shapes: [Option<Shape>; WelcomeTile::ALL.len()],
}

/// The size of a band and the theme that one drawing belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawingKey {
    width: u32,
    height: u32,
    theme: theme::ResolvedTheme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WelcomeKey {
    width: u32,
    theme: theme::ResolvedTheme,
    /// The rows of the section **Pick up where you left off**, which add their
    /// height to the block of the page.
    pickup_rows: usize,
}

/// Where every part of the page stands, for one width and one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct WelcomeMetrics {
    pub(in crate::app) columns: usize,
    pub(in crate::app) tile_width: f32,
    pub(in crate::app) tile_height: f32,
    /// The height of the tallest title of the four tiles, which every tile gives
    /// its own title, so the sentences of the four tiles stand on one line.
    pub(in crate::app) title_band: f32,
    /// The height of the tallest sentence of the four tiles, so the press of every
    /// tile keeps the same gap under it.
    pub(in crate::app) sentence_band: f32,
    pub(in crate::app) heading_height: f32,
    pub(in crate::app) lead_height: f32,
    /// The height of the section **Pick up where you left off**, or zero when the
    /// page shows no row.
    pub(in crate::app) pickup_height: f32,
    /// The height of everything the page draws, its pads excluded.
    pub(in crate::app) block_height: f32,
}

impl WelcomeMetrics {
    /// Measures the page that `width` and `pickup_rows` rows of the section give,
    /// with the wrapped texts laid out once.
    fn measure(painter: &Painter, width: f32, pickup_rows: usize) -> Self {
        let columns = welcome_columns(width);
        let tile = tile_width(width, columns);
        let inner_width = (tile - 2.0 * TILE_PAD).max(1.0);
        // A title or a sentence of one row more than the others must not push the
        // parts of its own tile onto each other, so every tile gives its title and
        // its sentence the room of the tallest one.
        let titles: f32 = WelcomeTile::ALL
            .iter()
            .map(|entry| {
                let wrapped = painter.layout_job(tile_title_job(
                    entry.title(),
                    &egui::FontId::proportional(TITLE_FONT),
                    theme::FG(),
                    inner_width,
                    Weight::Bold,
                ));
                wrapped.size().y
            })
            .fold(0.0, f32::max);
        let sentences: f32 = WelcomeTile::ALL
            .iter()
            .map(|entry| {
                let galley = painter.layout(
                    entry.sentence().to_owned(),
                    egui::FontId::proportional(SENTENCE_FONT),
                    theme::FG_SOFT(),
                    inner_width,
                );
                galley.size().y
            })
            .fold(0.0, f32::max);
        let title_band = titles.max(TITLE_FONT * TITLE_LINE_HEIGHT);
        let sentence_band = sentences.max(SENTENCE_FONT * SENTENCE_LINE_HEIGHT);
        #[allow(clippy::cast_precision_loss)]
        let rows = WelcomeTile::ALL.len().div_ceil(columns.max(1)) as f32;
        let tile_height = TILE_PAD
            + art::ART_HEIGHT
            + TILE_INNER_GAP
            + title_band
            + TILE_INNER_GAP
            + sentence_band
            + TILE_INNER_GAP
            + ACTION_FONT * ACTION_LINE_HEIGHT
            + TILE_PAD;
        let heading_height = HEADING_FONT * HEADING_LINE_HEIGHT;
        let lead = painter.layout(
            super::LEAD.to_owned(),
            egui::FontId::proportional(LEAD_FONT),
            theme::FG_SOFT(),
            HEADING_MAX_WIDTH.min(width.max(1.0)),
        );
        let lead_height = lead.size().y;
        let shortcuts_height = SHORTCUTS_FONT * 1.45;
        let pickup_height = if pickup_rows == 0 {
            0.0
        } else {
            pickup::SECTION_GAP + pickup::section_height(pickup_rows)
        };
        let block_height = HEADING_GAP
            + heading_height
            + HEADING_GAP
            + lead_height
            + HEADING_BLOCK_GAP
            + rows * tile_height
            + (rows - 1.0).max(0.0) * ROW_GAP
            + pickup_height
            + SHORTCUTS_GAP
            + shortcuts_height;
        Self {
            columns,
            tile_width: tile,
            tile_height,
            title_band,
            sentence_band,
            heading_height,
            lead_height,
            pickup_height,
            block_height,
        }
    }

    /// The row and the column of the tile at `index`.
    #[allow(clippy::cast_precision_loss)]
    pub(in crate::app) fn cell(&self, index: usize) -> (usize, usize) {
        let columns = self.columns.max(1);
        (index / columns, index % columns)
    }
}
