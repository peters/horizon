//! What the first start page remembers between frames: where its parts stand.
//!
//! The page is measured once per width, theme and count of the rows of the section
//! **Pick up where you left off**. The measurement lays out the wrapped titles and
//! sentences of the four tiles, which is the work that a still page would do on
//! every frame; the drawings of the tiles are a few dozen shapes built from
//! constants and are built on the frame that paints them.

use egui::Painter;

use super::art;
use super::layout::{ROW_GAP, tile_width, welcome_columns};
use super::pickup;
use super::tiles::WelcomeTile;
use super::{
    ACTION_FONT, ACTION_LINE_HEIGHT, HEADING_BLOCK_GAP, HEADING_FONT, HEADING_GAP, HEADING_LINE_HEIGHT,
    HEADING_MAX_WIDTH, LEAD_FONT, PAGE_PAD_TOP, SENTENCE_FONT, SENTENCE_LINE_HEIGHT, SHORTCUT_ROW, SHORTCUTS_FONT,
    SHORTCUTS_GAP, TILE_INNER_GAP, TILE_PAD, TITLE_FONT, TITLE_LINE_HEIGHT, tile_title_job,
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
}

impl WelcomeCache {
    /// The metrics of the page for `width`, `pickup_rows` rows of the section,
    /// `foot_rows` rows of shortcuts and a canvas `canvas_height` tall, measured
    /// with `painter`.
    ///
    /// The height of the canvas is part of the key because it decides the form of
    /// the page: a window that a person makes short first gives the drawings a
    /// smaller band and then no band at all.
    pub(in crate::app) fn metrics(
        &mut self,
        painter: &Painter,
        width: f32,
        pickup_rows: usize,
        foot_rows: usize,
        canvas_height: f32,
    ) -> WelcomeMetrics {
        let key = WelcomeKey {
            width: width.to_bits(),
            canvas: canvas_height.to_bits(),
            theme: theme::current_theme(),
            pickup_rows,
            foot_rows,
        };
        if self.key != Some(key) {
            self.key = Some(key);
            self.metrics = None;
        }
        *self
            .metrics
            .get_or_insert_with(|| WelcomeMetrics::measure(painter, width, pickup_rows, foot_rows, canvas_height))
    }
}

/// Whether a block of `block_height` points fits a canvas `canvas_height` tall
/// with the top pad of the page above it and the same pad under it.
fn fits(canvas_height: f32, block_height: f32) -> bool {
    canvas_height >= block_height + 2.0 * f32::from(PAGE_PAD_TOP)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WelcomeKey {
    width: u32,
    canvas: u32,
    theme: theme::ResolvedTheme,
    /// The rows of the section **Pick up where you left off**, which add their
    /// height to the block of the page.
    pickup_rows: usize,
    /// The rows of the shortcuts under the tiles: a narrow page takes more than
    /// one.
    foot_rows: usize,
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
    /// The height of the drawing band of a tile: [`art::ART_HEIGHT`], the compact
    /// height, or zero for the form of the page that draws no band.
    pub(in crate::app) art_height: f32,
    /// The height of everything the page draws, its pads excluded.
    pub(in crate::app) block_height: f32,
}

impl WelcomeMetrics {
    /// Measures the page that `width` and `pickup_rows` rows of the section give,
    /// with the wrapped texts laid out once, in the tallest form that a canvas
    /// `canvas_height` tall holds whole.
    fn measure(painter: &Painter, width: f32, pickup_rows: usize, foot_rows: usize, canvas_height: f32) -> Self {
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
        #[allow(clippy::cast_precision_loss)]
        let shortcuts_height = SHORTCUTS_FONT * 1.45 + foot_rows.saturating_sub(1) as f32 * SHORTCUT_ROW;
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
        let page = Self {
            columns,
            tile_width: tile,
            tile_height,
            title_band,
            sentence_band,
            heading_height,
            lead_height,
            pickup_height,
            art_height: art::ART_HEIGHT,
            block_height,
        };
        let art_height = page.art_height_that_fits(canvas_height);
        page.with_art_height(art_height)
    }

    /// The number of rows of tiles of the page.
    #[allow(clippy::cast_precision_loss)]
    fn rows(&self) -> f32 {
        WelcomeTile::ALL.len().div_ceil(self.columns.max(1)) as f32
    }

    /// The height of the drawing band of the first form of the page that a canvas
    /// `canvas_height` tall holds whole: the band of the mockup, then the compact
    /// band, then no band.
    ///
    /// A window that a person makes small keeps its four choices reachable this
    /// way. A canvas that holds none of the three forms gives no band, and the page
    /// scrolls.
    fn art_height_that_fits(&self, canvas_height: f32) -> f32 {
        let rows = self.rows();
        [art::ART_HEIGHT, art::ART_COMPACT_HEIGHT, 0.0]
            .into_iter()
            .find(|height| {
                let saved = rows * (art::ART_HEIGHT - height);
                fits(canvas_height, self.block_height - saved)
            })
            .unwrap_or(0.0)
    }

    /// The page with a drawing band of `art_height` points in every tile.
    fn with_art_height(self, art_height: f32) -> Self {
        let shorter = (art::ART_HEIGHT - art_height).max(0.0);
        Self {
            art_height,
            tile_height: self.tile_height - shorter,
            block_height: self.block_height - self.rows() * shorter,
            ..self
        }
    }

    /// The row and the column of the tile at `index`.
    #[allow(clippy::cast_precision_loss)]
    pub(in crate::app) fn cell(&self, index: usize) -> (usize, usize) {
        let columns = self.columns.max(1);
        (index / columns, index % columns)
    }
}
