//! The first start page: what a person can do with an empty window.
//!
//! The page has two forms, and the dock decides which one a window shows. A
//! window that shows the orchestrator dock draws the page that sends the person
//! to the front desk ([`front_desk`]): one heading, one sentence, three steps and
//! a link that turns the four starts over. A window without the dock keeps the
//! page of four tiles, which names four starts in plain words, each with a small
//! drawing, one sentence and the press that begins it. Both pages draw the row of
//! shortcuts underneath, and both show while the board has no workspace and no
//! panel, exactly where the old card showed; the same rule covers a window that a
//! person detached.
//!
//! A tile is a real control: it takes the keyboard with Tab, it starts on Enter or
//! Space, a reader hears its three lines, and it shows the hover and the focus
//! ring of its tokens. Nothing repaints while the pointer and the keyboard are
//! still: the drawings, the wrapped sentences and every position are built once
//! per width and theme, and a frame that changes nothing only paints them again.

mod art;
mod cache;
mod frame;
mod front_desk;
mod layout;
mod page;
mod pickup;
mod tiles;

pub(in crate::app) use tiles::WelcomeTile;

use egui::{
    Color32, Context, CornerRadius, CursorIcon, FontId, Id, Order, Painter, Pos2, Rect, Role, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::theme;
use crate::typeface::{self, Weight};

pub(in crate::app) use self::cache::WelcomeCache;
use self::cache::WelcomeMetrics;
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
/// The half-height of the chevron of the press of a tile.
const CHEVRON: f32 = 5.0;
/// The space between the press text and its chevron.
const CHEVRON_GAP: f32 = 8.0;
/// The space between two parts of the row of shortcuts.
const SHORTCUT_GAP: f32 = 18.0;
/// The distance from one row of shortcuts to the row under it, on a page that
/// is too narrow for one row.
const SHORTCUT_ROW: f32 = 26.0;
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

impl HorizonApp {
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

/// Draws the block of a first start page in `inner` for `content_height` points
/// of content, and gives the rectangle of the block to `paint`.
///
/// A page that is taller than the canvas scrolls inside it, and Page Up and Page
/// Down move one canvas in the sign of the scroll of the page. A page that the
/// canvas holds whole draws no scrollbar, and a block that fits is drawn whole,
/// whatever room the layer around the page has.
fn page_frame<R>(
    ctx: &Context,
    inner: Rect,
    content_height: f32,
    id: &str,
    paint: impl FnOnce(&mut Ui, Rect) -> R,
) -> Option<R> {
    let mut answer = None;
    egui::Area::new(Id::unique(id))
        .fixed_pos(inner.min)
        .order(Order::Foreground)
        // The page stands where the canvas puts it. egui keeps the area of the frame
        // before inside the screen, and the size of that area belongs to the window
        // of the frame before, so a window that becomes smaller would move the whole
        // page to the corner of the screen for one frame.
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_max_size(inner.size());
            if content_height > inner.height() {
                let step = page_step(ui, inner.height());
                if step != 0.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, step));
                }
            }
            egui::ScrollArea::vertical()
                .id_salt((id, "scroll"))
                .max_height(inner.height())
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(inner.width());
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let (content, _) = ui.allocate_exact_size(Vec2::new(inner.width(), content_height), Sense::hover());
                    answer = Some(paint(ui, content));
                });
        });
    answer
}

/// How far Page Up and Page Down move a page of a canvas `page` high, in the sign
/// that the scroll of the page takes: Page Down moves to the rows below the
/// canvas, Page Up back to the rows above it.
fn page_step(ui: &Ui, page: f32) -> f32 {
    let (down, up) = ui.input(|input| {
        (
            input.key_pressed(egui::Key::PageDown),
            input.key_pressed(egui::Key::PageUp),
        )
    });
    let mut step = 0.0;
    if down {
        step -= page;
    }
    if up {
        step += page;
    }
    step
}

/// The one line that says that this computer has no agent program, with the
/// programs that Horizon looked for.
fn no_agent_program(programs: &[String]) -> String {
    if programs.is_empty() {
        return "No preset in the settings starts an agent program. This is a terminal.".to_owned();
    }
    format!(
        "No agent program was found on this computer ({}). This is a terminal.",
        programs.join(", ")
    )
}

/// Whether the first page shows, from the counts of the board.
pub(in crate::app) fn welcome_page_shows(workspace_count: usize, panel_count: usize, overlays_visible: bool) -> bool {
    overlays_visible && workspace_count == 0 && panel_count == 0
}

/// One line of `text` at `size` and `weight`, as the mockups draw it.
fn heading_job(text: &str, size: f32, color: Color32, weight: Weight) -> egui::text::LayoutJob {
    egui::text::LayoutJob::single_section(
        text.to_owned(),
        typeface::format(&FontId::proportional(size), color, weight),
    )
}

/// The text of the page wrapped to `max_width` with every row centred, as the
/// line under the heading of the mockup is.
fn centred_job(text: &str, font: &FontId, color: Color32, max_width: f32, weight: Weight) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        halign: egui::Align::Center,
        wrap: egui::text::TextWrapping {
            max_width,
            ..egui::text::TextWrapping::default()
        },
        ..egui::text::LayoutJob::default()
    };
    job.append(text, 0.0, typeface::format(font, color, weight));
    job
}

/// The title of a tile wrapped to `max_width`, with every row at the left edge
/// of the tile, where its sentence and its action start.
///
/// A job with centred rows stands around its own origin, so a centred title
/// that is painted at the left edge of a tile hangs half outside the tile.
fn tile_title_job(text: &str, font: &FontId, color: Color32, max_width: f32, weight: Weight) -> egui::text::LayoutJob {
    let mut job = centred_job(text, font, color, max_width, weight);
    job.halign = egui::Align::LEFT;
    job
}

/// Paints one tile, and answers whether the person pressed it.
fn paint_tile(ui: &mut Ui, tile: WelcomeTile, rect: Rect, index: usize, metrics: WelcomeMetrics) -> bool {
    let response = ui.interact(rect, ui.make_persistent_id(("welcome_tile", index)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, tile.spoken()));
    let hovered = response.hovered();
    let focused = response.has_focus();
    // Tab may reach a tile that stands below the canvas of a small window, and the
    // page moves to the tile that the keyboard reached.
    if response.gained_focus() {
        response.scroll_to_me(Some(egui::Align::Center));
    }
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
    // The drawing of a tile is a score of shapes built from constants, so it is
    // built on the frame that paints it. The compact form of the page gives the
    // band no height, and then the tile keeps no drawing at all.
    let band = Rect::from_min_size(inner.min, Vec2::new(inner.width(), metrics.art_height));
    if band.height() > 0.0 {
        let mut drawing = Shape::Vec(art::local(tile, band.size()));
        drawing.translate(band.min.to_vec2());
        painter.add(drawing);
    }

    let mut y = band.max.y + TILE_INNER_GAP;
    let headline = painter.layout_job(tile_title_job(
        tile.title(),
        &FontId::proportional(TITLE_FONT),
        theme::FG(),
        inner.width(),
        Weight::Bold,
    ));
    painter.galley(Pos2::new(inner.min.x, y), headline, theme::FG());
    // The band of the tallest title keeps the sentence of every tile on one line,
    // and the band of the tallest sentence keeps the press of the tile at the
    // inner gap under it.
    y += metrics.title_band + TILE_INNER_GAP;
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
    let action = painter.layout_job(heading_job(tile.action(), ACTION_FONT, colour, Weight::Semibold));
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

#[cfg(test)]
impl HorizonApp {
    /// The height of the block of the first start page and of its section, for a
    /// test that pins the layout of the page.
    pub(in crate::app) fn welcome_block_heights_for_test(&mut self, ctx: &Context) -> (f32, f32) {
        let canvas = self.canvas_rect(ctx);
        let width = layout::page_width(canvas.width());
        let pickup = self.session_pickup.first_page();
        let measure = ctx.layer_painter(egui::LayerId::new(
            Order::Background,
            Id::unique("welcome_page_measure_test"),
        ));
        let foot_rows = page::shortcut_rows(&measure, width, &self.shortcut_lines());
        let metrics = self
            .welcome
            .metrics(&measure, width, pickup.len(), foot_rows, canvas.height());
        (metrics.block_height, metrics.pickup_height)
    }
}

#[cfg(test)]
mod tests;
