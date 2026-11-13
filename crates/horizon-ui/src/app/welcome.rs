//! The first start page: what a person can do with an empty window.
//!
//! The page replaces the one card that greeted a first start. It names four
//! starts in plain words, each with a small drawing, one sentence and the press
//! that begins it, and the row of shortcuts underneath names the keys of this
//! computer. It shows while the board has no workspace and no panel, exactly
//! where the old card showed, and the same rule covers a window that a person
//! detached.
//!
//! Every tile is a real control: it takes the keyboard with Tab, it starts on
//! Enter or Space, a reader hears its three lines, and it shows the hover and
//! the focus ring of its tokens. Nothing repaints while the pointer and the
//! keyboard are still: the drawings, the wrapped sentences and every position
//! are built once per width and theme, and a frame that changes nothing only
//! paints them again.

mod art;
mod layout;
mod tiles;

pub(in crate::app) use tiles::WelcomeTile;

use egui::{
    Color32, Context, CornerRadius, CursorIcon, FontId, Id, Order, Painter, Pos2, Rect, Role, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::theme;

use self::layout::{ROW_GAP, TILE_GAP, tile_width, welcome_columns};
use super::HorizonApp;

/// The space between the edge of the canvas and the page, in whole points.
const PAGE_PAD_SIDE: i8 = 60;
/// The space above the heading, in whole points.
const PAGE_PAD_TOP: i8 = 44;
/// The space below the row of shortcuts, in whole points.
const PAGE_PAD_BOTTOM: f32 = 28.0;
/// The widest line of the text under the heading.
const HEADING_MAX_WIDTH: f32 = 720.0;
/// The font of the heading.
const HEADING_FONT: f32 = 28.0;
/// The line height of the heading, as a multiple of its font size.
const HEADING_LINE_HEIGHT: f32 = 1.35;
/// The font of the line under the heading.
const LEAD_FONT: f32 = 14.0;
/// The space between the heading and its line.
const HEADING_GAP: f32 = 8.0;
/// The space between the heading block and the tiles.
const HEADING_BLOCK_GAP: f32 = 28.0;
/// The space between the tiles and the row of shortcuts.
const SHORTCUTS_GAP: f32 = 28.0;
/// The font of a shortcut line.
const SHORTCUTS_FONT: f32 = 12.0;
/// The space on both sides inside a tile.
const TILE_PAD: f32 = 20.0;
/// The space between two parts of a tile.
const TILE_INNER_GAP: f32 = 14.0;
/// The font of the title of a tile.
const TITLE_FONT: f32 = 16.0;
/// The line height of a title, as a multiple of its font size.
const TITLE_LINE_HEIGHT: f32 = 1.35;
/// The font of the sentence of a tile.
const SENTENCE_FONT: f32 = 12.5;
/// The line height of a sentence, as a multiple of its font size.
const SENTENCE_LINE_HEIGHT: f32 = 1.45;
/// The font of the press of a tile.
const ACTION_FONT: f32 = 12.5;
/// The line height of that press, as a multiple of its font size.
const ACTION_LINE_HEIGHT: f32 = 1.45;
/// The corner radius of a tile.
const TILE_RADIUS: u8 = 16;
/// The width of the outline of a tile.
const TILE_OUTLINE: f32 = 1.0;
/// The width of the ring that says where the keyboard is.
const FOCUS_RING: f32 = 2.0;
/// How far a second pass of the heading stands to the right of the first, in
/// whole points: the bundled face has one weight, and the two passes together
/// give the heavy heading of the mockup.
const HEAVY_OFFSET: f32 = 0.6;
/// The half-height of the chevron of the press of a tile.
const CHEVRON: f32 = 5.0;
/// The space between the press text and its chevron.
const CHEVRON_GAP: f32 = 8.0;
/// The space between two parts of the row of shortcuts.
const SHORTCUT_GAP: f32 = 18.0;
/// The corner radius of the chip of one key.
const KEY_RADIUS: u8 = 6;
/// The font of a key.
const KEY_FONT: f32 = 10.0;

/// The heading of the page.
pub(in crate::app) const HEADING: &str = "What do you want to do?";
/// The line under the heading.
pub(in crate::app) const LEAD: &str = "Horizon is one canvas for your terminals, your agents and your machines. Choose a \
                                         start. An agent does the set-up and shows you each step.";
/// The line that starts the row of shortcuts.
pub(in crate::app) const SHORTCUTS_LEAD: &str = "You know Horizon already?";

/// What the page remembers, so a still page lays nothing out again.
#[derive(Default)]
pub(in crate::app) struct WelcomeCache {
    /// The width and the theme that the metrics and the drawings belong to.
    key: Option<WelcomeKey>,
    metrics: Option<WelcomeMetrics>,
    drawings: [Option<Shape>; WelcomeTile::ALL.len()],
}

impl WelcomeCache {
    /// The metrics of the page for `width`, measured with `painter`.
    fn metrics(&mut self, painter: &Painter, width: f32) -> WelcomeMetrics {
        let key = WelcomeKey {
            width: width.to_bits(),
            theme: theme::current_theme(),
        };
        if self.key != Some(key) {
            let metrics = WelcomeMetrics::measure(painter, width);
            self.key = Some(key);
            self.metrics = Some(metrics);
            self.drawings = Default::default();
        }
        self.metrics.unwrap_or_else(|| WelcomeMetrics::measure(painter, width))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WelcomeKey {
    width: u32,
    theme: theme::ResolvedTheme,
}

/// Where every part of the page stands, for one width and one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WelcomeMetrics {
    columns: usize,
    tile_width: f32,
    tile_height: f32,
    heading_height: f32,
    lead_height: f32,
    /// The height of everything the page draws, its pads excluded.
    block_height: f32,
}

impl WelcomeMetrics {
    /// Measures the page that `width` gives, with the wrapped texts laid out
    /// once.
    fn measure(painter: &Painter, width: f32) -> Self {
        let columns = welcome_columns(width);
        let tile = tile_width(width, columns);
        let sentence_width = (tile - 2.0 * TILE_PAD).max(1.0);
        let sentences: f32 = WelcomeTile::ALL
            .iter()
            .map(|entry| {
                let galley = painter.layout(
                    entry.sentence().to_owned(),
                    FontId::proportional(SENTENCE_FONT),
                    theme::FG_SOFT(),
                    sentence_width,
                );
                galley.size().y
            })
            .fold(0.0, f32::max);
        #[allow(clippy::cast_precision_loss)]
        let rows = WelcomeTile::ALL.len().div_ceil(columns.max(1)) as f32;
        let tile_height = TILE_PAD
            + art::ART_HEIGHT
            + TILE_INNER_GAP
            + TITLE_FONT * TITLE_LINE_HEIGHT
            + TILE_INNER_GAP
            + sentences.max(SENTENCE_FONT * SENTENCE_LINE_HEIGHT)
            + TILE_INNER_GAP
            + ACTION_FONT * ACTION_LINE_HEIGHT
            + TILE_PAD;
        let heading_height = HEADING_FONT * HEADING_LINE_HEIGHT;
        let lead = painter.layout(
            LEAD.to_owned(),
            FontId::proportional(LEAD_FONT),
            theme::FG_SOFT(),
            HEADING_MAX_WIDTH.min(width.max(1.0)),
        );
        let lead_height = lead.size().y;
        let shortcuts_height = SHORTCUTS_FONT * 1.45;
        let block_height = HEADING_GAP
            + heading_height
            + HEADING_GAP
            + lead_height
            + HEADING_BLOCK_GAP
            + rows * tile_height
            + (rows - 1.0).max(0.0) * ROW_GAP
            + SHORTCUTS_GAP
            + shortcuts_height;
        Self {
            columns,
            tile_width: tile,
            tile_height,
            heading_height,
            lead_height,
            block_height,
        }
    }

    /// The row and the column of the tile at `index`.
    #[allow(clippy::cast_precision_loss)]
    fn cell(&self, index: usize) -> (usize, usize) {
        let columns = self.columns.max(1);
        (index / columns, index % columns)
    }
}

impl HorizonApp {
    /// Draws the first start page, while the board holds no workspace and no
    /// panel.
    pub(super) fn render_welcome_page(&mut self, ctx: &Context) {
        if !self.welcome_page_visible() {
            return;
        }
        let canvas = self.canvas_rect(ctx);
        let pad_side = f32::from(PAGE_PAD_SIDE);
        let width = (canvas.width() - 2.0 * pad_side).max(1.0);
        // The block is measured before it is shown, because a block that fits
        // the canvas is centred in it and one that does not starts at the top.
        let measure = ctx.layer_painter(egui::LayerId::new(
            Order::Background,
            Id::unique("welcome_page_measure"),
        ));
        let metrics = self.welcome.metrics(&measure, width);
        let top = page_top(canvas.height(), metrics.block_height) + canvas.min.y;
        let inner = Rect::from_min_max(
            Pos2::new(canvas.min.x + pad_side, top),
            Pos2::new((canvas.max.x - pad_side).max(canvas.min.x), canvas.max.y),
        );
        let shortcut_lines = self.shortcut_lines();
        let mut pressed = None;

        egui::Area::new(Id::unique("welcome_page"))
            .fixed_pos(inner.min)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                // The page owns the rect that the canvas gives it, so a tall
                // block scrolls inside the canvas and a block that fits is drawn
                // whole, whatever room the layer around the page has.
                ui.set_max_size(inner.size());
                egui::ScrollArea::vertical()
                    .id_salt("welcome_page_scroll")
                    .max_height(inner.height())
                    // A block that fits draws no scrollbar, and the bar that a
                    // taller block needs takes no room from the tiles.
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(inner.width());
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let (content, _) = ui.allocate_exact_size(
                            Vec2::new(inner.width(), metrics.block_height + PAGE_PAD_BOTTOM),
                            Sense::hover(),
                        );
                        pressed = paint_page(ui, &mut self.welcome, content, metrics, &shortcut_lines);
                    });
            });

        if let Some(tile) = pressed {
            self.start_welcome_tile(ctx, tile);
        }
    }

    /// The keys of this computer, as the row under the tiles says them.
    fn shortcut_lines(&self) -> Vec<(String, String)> {
        let primary = super::util::primary_shortcut_label();
        vec![
            (
                "New terminal".to_owned(),
                self.shortcuts.new_terminal.display_label(primary),
            ),
            (
                "Quick Nav".to_owned(),
                self.shortcuts.command_palette.display_label(primary),
            ),
            (
                "Machines you have".to_owned(),
                self.shortcuts.open_remote_hosts.display_label(primary),
            ),
        ]
    }

    /// Starts what a tile of the first page names.
    fn start_welcome_tile(&mut self, ctx: &Context, tile: WelcomeTile) {
        match tile.goal() {
            Some(goal) => self.press_set_up(ctx, goal),
            // The first tile opens the panel of the person's first preset, as
            // the New Terminal button of the canvas did.
            None => {
                if let Some(preset) = self.presets.first().cloned() {
                    let workspace_id = self.ensure_workspace_visible(ctx);
                    self.add_panel_to_workspace(ctx, workspace_id, preset, None);
                } else {
                    self.create_panel(ctx);
                }
            }
        }
    }

    /// Whether the first page shows: no workspace, no panel, and no dialog over
    /// the canvas.
    fn welcome_page_visible(&self) -> bool {
        welcome_page_shows(
            self.board.workspaces.len(),
            self.board.panels.len(),
            self.fixed_overlays_visible(),
        )
    }
}

/// The distance from the top of the canvas to the top of the block of the page.
///
/// A block that fits between the top pads of the page is centred in the height
/// of the canvas, so the page does not sit against the top with an empty lower
/// half. A block that needs more room keeps the top pad and scrolls under it.
fn page_top(canvas_height: f32, block_height: f32) -> f32 {
    let top = f32::from(PAGE_PAD_TOP);
    if canvas_height >= block_height + 2.0 * top {
        (canvas_height - block_height) * 0.5
    } else {
        top
    }
}

/// Whether the first page shows, from the counts of the board.
pub(in crate::app) fn welcome_page_shows(workspace_count: usize, panel_count: usize, overlays_visible: bool) -> bool {
    overlays_visible && workspace_count == 0 && panel_count == 0
}

/// Paints the whole page inside `content` and answers the tile that the person
/// pressed.
fn paint_page(
    ui: &mut Ui,
    cache: &mut WelcomeCache,
    content: Rect,
    metrics: WelcomeMetrics,
    shortcuts: &[(String, String)],
) -> Option<WelcomeTile> {
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
        if paint_tile(ui, cache, *tile, rect, index) {
            pressed = Some(*tile);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let rows = WelcomeTile::ALL.len().div_ceil(metrics.columns.max(1)) as f32;
    let below = y + rows * metrics.tile_height + (rows - 1.0).max(0.0) * ROW_GAP + SHORTCUTS_GAP;
    paint_shortcuts(&painter, content, below, shortcuts);
    pressed
}

/// Paints the heading and its line, and answers the top of the tiles.
fn paint_heading(painter: &Painter, content: Rect, metrics: WelcomeMetrics, y: f32) -> f32 {
    let center = content.center().x;
    let heading = painter.layout_no_wrap(HEADING.to_owned(), FontId::proportional(HEADING_FONT), theme::FG());
    let left = center - heading.size().x * 0.5;
    paint_heavy(painter, Pos2::new(left, y), heading);
    let lead_top = y + metrics.heading_height + HEADING_GAP;
    let lead = painter.layout_job(centred_job(
        LEAD,
        &FontId::proportional(LEAD_FONT),
        theme::FG_SOFT(),
        HEADING_MAX_WIDTH.min(content.width().max(1.0)),
    ));
    // The rows of a centred job stand around its own origin, so the galley is
    // drawn on the middle line of the page.
    painter.galley(Pos2::new(center, lead_top), lead, theme::FG_SOFT());
    lead_top + metrics.lead_height + HEADING_BLOCK_GAP - HEADING_GAP
}

/// Paints one line with the weight of a heading of the mockup.
///
/// The bundled face carries one weight, so the line is painted twice by a hair
/// instead of once: the page asks for a strong heading and no second face is
/// bundled for it.
fn paint_heavy(painter: &Painter, pos: Pos2, galley: std::sync::Arc<egui::Galley>) {
    painter.galley(pos, galley.clone(), theme::FG());
    painter.galley(pos + Vec2::new(HEAVY_OFFSET, 0.0), galley, theme::FG());
}

/// The text of the page wrapped to `max_width` with every row centred, as the
/// line under the heading of the mockup is.
fn centred_job(text: &str, font: &FontId, color: Color32, max_width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        halign: egui::Align::Center,
        wrap: egui::text::TextWrapping {
            max_width,
            ..egui::text::TextWrapping::default()
        },
        ..egui::text::LayoutJob::default()
    };
    job.append(
        text,
        0.0,
        egui::text::TextFormat {
            font_id: font.clone(),
            color,
            ..egui::text::TextFormat::default()
        },
    );
    job
}

/// Paints one tile, and answers whether the person pressed it.
fn paint_tile(ui: &mut Ui, cache: &mut WelcomeCache, tile: WelcomeTile, rect: Rect, index: usize) -> bool {
    let response = ui.interact(rect, ui.make_persistent_id(("welcome_tile", index)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, tile.spoken()));
    let hovered = response.hovered();
    let focused = response.has_focus();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    let radius = CornerRadius::same(TILE_RADIUS);
    let fill = if hovered {
        theme::blend(theme::PANEL_BG(), theme::ACCENT(), 0.06)
    } else {
        theme::PANEL_BG()
    };
    painter.rect_filled(rect, radius, fill);
    let outline = if hovered || focused {
        theme::ACCENT()
    } else {
        theme::BORDER_SUBTLE()
    };
    painter.rect_stroke(rect, radius, Stroke::new(TILE_OUTLINE, outline), StrokeKind::Inside);
    if focused {
        painter.rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(TILE_RADIUS + 2),
            Stroke::new(FOCUS_RING, theme::FG()),
            StrokeKind::Outside,
        );
    }

    let inner = Rect::from_min_max(
        Pos2::new(rect.min.x + TILE_PAD, rect.min.y + TILE_PAD),
        Pos2::new(rect.max.x - TILE_PAD, rect.max.y - TILE_PAD),
    );
    // The drawing of a tile is static, so it is built once per width and theme
    // and painted again on every frame that asks for one.
    let band = Rect::from_min_size(inner.min, Vec2::new(inner.width(), art::ART_HEIGHT));
    let drawing = cache.drawings[index].get_or_insert_with(|| Shape::Vec(art::art(tile, band)));
    painter.add(drawing.clone());

    let mut y = band.max.y + TILE_INNER_GAP;
    let headline = painter.layout(
        tile.title().to_owned(),
        FontId::proportional(TITLE_FONT),
        theme::FG(),
        inner.width(),
    );
    painter.galley(Pos2::new(inner.min.x, y), headline.clone(), theme::FG());
    y += headline.size().y + TILE_INNER_GAP;
    let sentence = painter.layout(
        tile.sentence().to_owned(),
        FontId::proportional(SENTENCE_FONT),
        theme::FG_SOFT(),
        inner.width(),
    );
    painter.galley(Pos2::new(inner.min.x, y), sentence, theme::FG_SOFT());

    paint_action(painter, inner, tile);
    response.clicked()
}

/// Paints the press of a tile on its bottom edge, with the chevron that says
/// where it goes.
fn paint_action(painter: &Painter, inner: Rect, tile: WelcomeTile) {
    let colour = action_colour();
    let action = painter.layout_no_wrap(tile.action().to_owned(), FontId::proportional(ACTION_FONT), colour);
    let top = inner.max.y - action.size().y;
    let width = action.size().x;
    painter.galley(Pos2::new(inner.min.x, top), action, colour);
    let left = inner.min.x + width + CHEVRON_GAP;
    let center = top + ACTION_FONT * ACTION_LINE_HEIGHT * 0.5;
    let ink = Stroke::new(1.6, colour);
    painter.line_segment(
        [Pos2::new(left, center - CHEVRON), Pos2::new(left + CHEVRON, center)],
        ink,
    );
    painter.line_segment(
        [Pos2::new(left + CHEVRON, center), Pos2::new(left, center + CHEVRON)],
        ink,
    );
}

/// The colour of the press line of a tile: the accent, lifted towards the
/// foreground so it reads on both themes.
fn action_colour() -> Color32 {
    theme::blend(theme::ACCENT(), theme::FG(), 0.30)
}

/// Paints the row of shortcuts under the tiles.
fn paint_shortcuts(painter: &Painter, content: Rect, top: f32, lines: &[(String, String)]) {
    let lead = painter.layout_no_wrap(
        SHORTCUTS_LEAD.to_owned(),
        FontId::proportional(SHORTCUTS_FONT),
        theme::FG_SOFT(),
    );
    let height = lead.size().y;
    let mut x = content.min.x;
    painter.galley(Pos2::new(x, top), lead.clone(), theme::FG_SOFT());
    x += lead.size().x + SHORTCUT_GAP;
    for (label, keys) in lines {
        let text = painter.layout_no_wrap(label.clone(), FontId::proportional(SHORTCUTS_FONT), theme::FG_SOFT());
        painter.galley(Pos2::new(x, top), text.clone(), theme::FG_SOFT());
        x += text.size().x + 6.0;
        let key = painter.layout_no_wrap(keys.clone(), FontId::monospace(KEY_FONT), theme::FG_SOFT());
        let chip = Rect::from_min_size(
            Pos2::new(x, top + (height - key.size().y) * 0.5 - 1.0),
            key.size() + Vec2::new(12.0, 4.0),
        );
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
}

#[cfg(test)]
mod tests;
