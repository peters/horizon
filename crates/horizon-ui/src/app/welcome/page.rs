//! The page of four tiles: the heading with its line, the four starts, the section
//! that picks an earlier session up and the row of shortcuts under them.
//!
//! The page reads its metrics from `cache` and draws inside the block that the frame
//! measures, so a frame that changes nothing only paints the block again.

use egui::{Context, CornerRadius, FontId, Id, Order, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

use crate::theme;
use crate::typeface::Weight;

use super::super::HorizonApp;
use super::super::session_pickup::SessionPickup;
use super::cache::WelcomeMetrics;
use super::layout::{ROW_GAP, TILE_GAP, page_width};
use super::pickup as pickup_section;
use super::pickup::PickupSectionPress;
use super::{
    HEADING, HEADING_BLOCK_GAP, HEADING_FONT, HEADING_GAP, HEADING_MAX_WIDTH, KEY_FONT, KEY_RADIUS, LEAD, LEAD_FONT,
    PAGE_PAD_BOTTOM, SHORTCUT_GAP, SHORTCUT_ROW, SHORTCUTS_FONT, SHORTCUTS_GAP, SHORTCUTS_LEAD, WelcomeTile,
    centred_job, heading_job, page_frame, page_top, paint_tile,
};

impl HorizonApp {
    /// Draws the page of four tiles, which a window without the dock shows.
    pub(super) fn render_tiles_page(&mut self, ctx: &Context) {
        if !self.welcome_page_visible() {
            return;
        }
        let canvas = self.canvas_rect(ctx);
        let width = page_width(canvas.width());
        // The rows of the section are read before the block is measured, because
        // they are part of its height.
        let pickup = self.session_pickup.first_page();
        // The block is measured before it is shown, because a block that fits
        // the canvas is centred in it and one that does not starts at the top.
        let measure = ctx.layer_painter(egui::LayerId::new(
            Order::Background,
            Id::unique("welcome_page_measure"),
        ));
        let shortcut_lines = self.shortcut_lines();
        let foot_rows = shortcut_rows(&measure, width, &shortcut_lines);
        let metrics = self
            .welcome
            .metrics(&measure, width, pickup.len(), foot_rows, canvas.height());
        let top = page_top(canvas.height(), metrics.block_height) + canvas.min.y;
        // A canvas that is wider than the reading width of the page keeps the same
        // air on both sides of the block.
        let left = canvas.min.x + (canvas.width() - width) * 0.5;
        let inner = Rect::from_min_max(Pos2::new(left, top), Pos2::new(left + width, canvas.max.y));
        let painted = page_frame(
            ctx,
            inner,
            metrics.block_height + PAGE_PAD_BOTTOM,
            "welcome_page",
            |ui, content| paint_page(ui, content, metrics, &shortcut_lines, &pickup, &self.session_pickup),
        );
        let painted = painted.unwrap_or_default();

        if let Some(tile) = painted.tile {
            self.start_welcome_tile(ctx, tile);
        }
        if let Some(press) = painted.pickup {
            self.apply_pickup_press(ctx, &press);
        }
    }
}

/// What a frame of the page pressed.
#[derive(Default)]
struct WelcomePagePress {
    /// The tile that the person pressed.
    tile: Option<WelcomeTile>,
    /// The row of the section that the person pressed, or its control `Show all`.
    pickup: Option<PickupSectionPress>,
}

/// Paints the whole page inside `content` and answers the presses of the person.
fn paint_page(
    ui: &mut Ui,
    content: Rect,
    metrics: WelcomeMetrics,
    shortcuts: &[(String, String)],
    pickup: &[horizon_core::PickupFolder],
    state: &SessionPickup,
) -> WelcomePagePress {
    let painter = ui.painter().clone();
    let mut y = content.min.y + HEADING_GAP;
    y = paint_heading(&painter, content, metrics, y);
    let mut pressed = None;
    for (index, tile) in WelcomeTile::ALL.iter().enumerate() {
        let (row, column) = metrics.cell(index);
        #[allow(clippy::cast_precision_loss)]
        let x = content.min.x + column as f32 * (metrics.tile_width + TILE_GAP);
        #[allow(clippy::cast_precision_loss)]
        let top = y + row as f32 * (metrics.tile_height + ROW_GAP);
        let rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(metrics.tile_width, metrics.tile_height));
        if paint_tile(ui, *tile, rect, index, metrics) {
            pressed = Some(*tile);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let rows = WelcomeTile::ALL.len().div_ceil(metrics.columns.max(1)) as f32;
    let below_tiles = y + rows * metrics.tile_height + (rows - 1.0).max(0.0) * ROW_GAP;
    let pickup_press = if metrics.pickup_height > 0.0 {
        pickup_section::paint_section(ui, content, below_tiles + pickup_section::SECTION_GAP, pickup, |row| {
            state.is_confirmed(row.kind, &row.session_id)
        })
    } else {
        None
    };
    let foot = below_tiles + metrics.pickup_height + SHORTCUTS_GAP;
    let _ = paint_shortcuts(&painter, content, foot, shortcuts);
    WelcomePagePress {
        tile: pressed,
        pickup: pickup_press,
    }
}
/// Paints the heading and its line, and answers the top of the tiles.
fn paint_heading(painter: &Painter, content: Rect, metrics: WelcomeMetrics, y: f32) -> f32 {
    let center = content.center().x;
    let heading = painter.layout_job(heading_job(HEADING, HEADING_FONT, theme::FG(), Weight::Black));
    let left = center - heading.size().x * 0.5;
    painter.galley(Pos2::new(left, y), heading, theme::FG());
    let lead_top = y + metrics.heading_height + HEADING_GAP;
    let lead = painter.layout_job(centred_job(
        LEAD,
        &FontId::proportional(LEAD_FONT),
        theme::FG_SOFT(),
        HEADING_MAX_WIDTH.min(content.width().max(1.0)),
        Weight::Regular,
    ));
    // The rows of a centred job stand around its own origin, so the galley is
    // drawn on the middle line of the page.
    painter.galley(Pos2::new(center, lead_top), lead, theme::FG_SOFT());
    lead_top + metrics.lead_height + HEADING_BLOCK_GAP - HEADING_GAP
}

/// The rows that the shortcuts take on a page `width` wide. The painter of the
/// measure is not visible, so the measure paints nothing.
pub(super) fn shortcut_rows(measure: &Painter, width: f32, lines: &[(String, String)]) -> usize {
    let mut unseen = measure.clone();
    unseen.set_invisible();
    let page = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 0.0));
    paint_shortcuts(&unseen, page, 0.0, lines)
}
/// Paints the shortcuts under the tiles, and answers the rows that they take.
///
/// A name and its key that do not fit the width of the page start the next row,
/// so a narrow window cuts no part of the line at its right edge.
fn paint_shortcuts(painter: &Painter, content: Rect, top: f32, lines: &[(String, String)]) -> usize {
    let lead = painter.layout_no_wrap(
        SHORTCUTS_LEAD.to_owned(),
        FontId::proportional(SHORTCUTS_FONT),
        theme::FG_SOFT(),
    );
    let height = lead.size().y;
    let (mut x, mut y, mut rows) = (content.min.x, top, 1);
    painter.galley(Pos2::new(x, y), lead.clone(), theme::FG_SOFT());
    x += lead.size().x + SHORTCUT_GAP;
    for (label, keys) in lines {
        let text = painter.layout_no_wrap(label.clone(), FontId::proportional(SHORTCUTS_FONT), theme::FG_SOFT());
        let key = painter.layout_no_wrap(keys.clone(), FontId::monospace(KEY_FONT), theme::FG_SOFT());
        let chip_size = key.size() + Vec2::new(12.0, 4.0);
        if x > content.min.x && x + text.size().x + 6.0 + chip_size.x > content.max.x {
            x = content.min.x;
            y += SHORTCUT_ROW;
            rows += 1;
        }
        painter.galley(Pos2::new(x, y), text.clone(), theme::FG_SOFT());
        x += text.size().x + 6.0;
        let chip = Rect::from_min_size(Pos2::new(x, y + (height - key.size().y) * 0.5 - 1.0), chip_size);
        painter.rect_filled(chip, CornerRadius::same(KEY_RADIUS), theme::PANEL_BG());
        painter.rect_stroke(
            chip,
            CornerRadius::same(KEY_RADIUS),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        painter.galley(chip.min + Vec2::new(6.0, 2.0), key, theme::FG_SOFT());
        x = chip.max.x + SHORTCUT_GAP;
    }
    rows
}
