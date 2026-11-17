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
mod cache;
mod layout;
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
use self::layout::{ROW_GAP, TILE_GAP, page_width};
use self::pickup as pickup_section;
use super::HorizonApp;
use super::session_pickup::SessionPickup;
use super::setup;
use horizon_core::setup::{FolderPanel, find_program, setup_agents};

use self::pickup::PickupSectionPress;

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
    /// Draws the first start page, while the board holds no workspace and no
    /// panel.
    pub(super) fn render_welcome_page(&mut self, ctx: &Context) {
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
        let mut pressed = None;
        let mut pickup_press = None;

        egui::Area::new(Id::unique("welcome_page"))
            .fixed_pos(inner.min)
            .order(Order::Foreground)
            // The page stands where the canvas puts it. egui keeps the area of the
            // frame before inside the screen, and the size of that area belongs to
            // the window of the frame before, so a window that becomes smaller
            // would move the whole page to the corner of the screen for one frame.
            .constrain(false)
            .show(ctx, |ui| {
                // The page owns the rect that the canvas gives it, so a tall
                // block scrolls inside the canvas and a block that fits is drawn
                // whole, whatever room the layer around the page has.
                ui.set_max_size(inner.size());
                // A page that is taller than the canvas moves one canvas for Page
                // Up and Page Down, in the sign of the scroll of the page.
                if metrics.block_height + PAGE_PAD_BOTTOM > inner.height() {
                    let step = page_step(ui, inner.height());
                    if step != 0.0 {
                        ui.scroll_with_delta(Vec2::new(0.0, step));
                    }
                }
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
                        let painted = paint_page(ui, content, metrics, &shortcut_lines, &pickup, &self.session_pickup);
                        pressed = painted.tile;
                        pickup_press = painted.pickup;
                    });
            });

        if let Some(tile) = pressed {
            self.start_welcome_tile(ctx, tile);
        }
        if let Some(press) = pickup_press {
            self.apply_pickup_press(ctx, &press);
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
            None => self.start_agent_tile(ctx),
        }
    }

    /// The first tile starts a panel of the first agent preset, in the order of
    /// the settings, whose program this computer has.
    ///
    /// A computer with no agent program keeps the first preset of the settings,
    /// and one line says that this is a terminal. The search reads the folders of
    /// the `PATH`, so it runs on the coordination worker and the panel opens on a
    /// later frame.
    fn start_agent_tile(&mut self, ctx: &Context) {
        if self.presets.is_empty() {
            self.create_panel(ctx);
            return;
        }
        let workspace_id = self.ensure_workspace_visible(ctx);
        let presets = self.presets.clone();
        let programs: Vec<String> = setup_agents(&presets).into_iter().map(|agent| agent.program).collect();
        let searched = programs.clone();
        let opening = ctx.clone();
        self.browser_create_host.io.then(
            move || {
                searched
                    .iter()
                    .map(|program| find_program(program, None).is_some())
                    .collect::<Vec<bool>>()
            },
            move |app, found| {
                let found = found.unwrap_or_default();
                let preset = match setup::installed_agent(&presets, &found) {
                    FolderPanel::Preset(index) => presets.get(index).cloned(),
                    FolderPanel::Terminal => {
                        app.set_sidebar_plain_notice(no_agent_program(&programs));
                        presets.first().cloned()
                    }
                };
                match preset {
                    Some(preset) => app.add_panel_to_workspace(&opening, workspace_id, preset, None),
                    None => app.create_panel(&opening),
                }
            },
        );
    }

    /// Whether the first page shows: no workspace, no panel, and no dialog over
    /// the canvas.
    pub(in crate::app) fn welcome_page_visible(&self) -> bool {
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

/// The rows that the shortcuts take on a page `width` wide. The painter of the
/// measure is not visible, so the measure paints nothing.
fn shortcut_rows(measure: &Painter, width: f32, lines: &[(String, String)]) -> usize {
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

#[cfg(test)]
impl HorizonApp {
    /// The height of the block of the first start page and of its section, for a
    /// test that pins the layout of the page.
    pub(in crate::app) fn welcome_block_heights_for_test(&mut self, ctx: &Context) -> (f32, f32) {
        let canvas = self.canvas_rect(ctx);
        let width = page_width(canvas.width());
        let pickup = self.session_pickup.first_page();
        let measure = ctx.layer_painter(egui::LayerId::new(
            Order::Background,
            Id::unique("welcome_page_measure_test"),
        ));
        let foot_rows = shortcut_rows(&measure, width, &self.shortcut_lines());
        let metrics = self
            .welcome
            .metrics(&measure, width, pickup.len(), foot_rows, canvas.height());
        (metrics.block_height, metrics.pickup_height)
    }
}

#[cfg(test)]
mod tests;
