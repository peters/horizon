//! The first start page of a window that shows the orchestrator dock.
//!
//! The page of four tiles answers a window that has no dock. A window that shows
//! the dock at its right edge draws this page instead: a calm page that sends the
//! person to the front desk. It names the start in one heading, one sentence and
//! three steps, holds the four tiles of the other page behind the link
//! `More ways to start`, and keeps the row of shortcuts of this computer
//! underneath.
//!
//! The page is drawn in the rectangle of the canvas, which the dock leaves at its
//! left, and its block is centred in that rectangle. Every text is laid out once
//! per width, theme, canvas height and state of the link, so a still page lays
//! nothing out again and asks for no repaint.

use std::sync::Arc;

use egui::{
    Context, CornerRadius, CursorIcon, FontId, Galley, Id, LayerId, Order, Painter, Pos2, Rect, Role, Sense, Stroke,
    StrokeKind, Ui, Vec2, WidgetInfo,
};

use super::cache::WelcomeMetrics;
use super::layout::{ROW_GAP, TILE_GAP, page_width};
use super::tiles::WelcomeTile;
use super::{
    ACTION_FONT, CHEVRON, CHEVRON_GAP, HEADING_FONT, HEADING_GAP, KEY_FONT, KEY_RADIUS, LEAD_FONT, PAGE_PAD_BOTTOM,
    PAGE_PAD_TOP, SHORTCUT_GAP, SHORTCUT_ROW, SHORTCUTS_FONT, SHORTCUTS_GAP, SHORTCUTS_LEAD, centred_job, heading_job,
    page_frame, tile_title_job,
};
use crate::app::util::usize_to_f32;
use crate::theme;
use crate::typeface::Weight;

/// The heading of the page.
pub(in crate::app) const HEADING: &str = "Start at the front desk";
/// The line under the heading.
pub(in crate::app) const LEAD: &str = "Write what you want done in the conversation at the right. Each task becomes a work \
                                       item with its own orchestrator, and its work shows on this canvas.";
/// The link while the page holds the four tiles behind it.
pub(in crate::app) const MORE: &str = "More ways to start";
/// The same link while the four tiles stand under it.
pub(in crate::app) const FEWER: &str = "Fewer ways to start";

/// The widest line of the text under the heading.
const LEAD_MAX_WIDTH: f32 = 560.0;
/// The space between the line under the heading and the steps, and between the
/// steps and the link.
const BLOCK_GAP: f32 = 32.0;
/// The width of one step.
const STEP_WIDTH: f32 = 200.0;
/// The space between two steps of the row.
const STEP_GAP: f32 = 40.0;
/// The air that the row of steps needs at each side of the block.
const STEP_ROW_PAD: f32 = 24.0;
/// The space between two steps of the column.
const STEP_COLUMN_GAP: f32 = 20.0;
/// The side of the chip of a step, and its radius.
const CHIP: f32 = 22.0;
const CHIP_RADIUS: u8 = 11;
/// The font of the digit of a chip.
const CHIP_DIGIT_FONT: f32 = 11.5;
/// The space between the chip of a step and its title.
const STEP_TITLE_GAP: f32 = 10.0;
/// The font of the title of a step.
const STEP_TITLE_FONT: f32 = 13.5;
/// The space between the title of a step and its sentence.
const STEP_SENTENCE_GAP: f32 = 4.0;
/// The font of the sentence of a step.
const STEP_SENTENCE_FONT: f32 = 12.5;
/// The most rows of a sentence of a step.
const STEP_SENTENCE_ROWS: usize = 3;
/// The distance from the right edge of a chip to the line that joins it to the
/// chip of the next step, and the same distance to that chip.
const JOIN_INSET: f32 = 10.0;
/// The space between the link and the four tiles under it.
const TILES_GAP: f32 = 24.0;
/// Where the middle of the block stands in the height of the canvas.
const BLOCK_CENTRE: f32 = 0.46;

/// One step of the page: the title under its chip and the sentence under that.
pub(in crate::app) struct Step {
    pub(in crate::app) title: &'static str,
    pub(in crate::app) sentence: &'static str,
}

/// The three steps of the page, in their order.
pub(in crate::app) const STEPS: [Step; 3] = [
    Step {
        title: "Write a task",
        sentence: "One message can hold several tasks. Say what done means.",
    },
    Step {
        title: "Press Start",
        sentence: "The front desk shows the work items first. Nothing starts and nothing costs before your press.",
    },
    Step {
        title: "Answer what needs you",
        sentence: "A question shows at the top of the sidebar with its buttons, wherever you look.",
    },
];

/// The text of one step, laid out once.
#[derive(Clone)]
struct StepText {
    /// The number of the chip, centred in it.
    digit: Arc<Galley>,
    title: Arc<Galley>,
    sentence: Arc<Galley>,
    /// Where the title and the sentence start, from the top of the step.
    title_top: f32,
    sentence_top: f32,
}

/// One row of the shortcuts of this computer: a name, its key and the chip of
/// the key.
#[derive(Clone)]
struct FootPart {
    label: Arc<Galley>,
    key: Arc<Galley>,
    label_pos: Pos2,
    chip: Rect,
}

/// The row of shortcuts of this computer, laid out once.
#[derive(Clone)]
struct Foot {
    lead: Arc<Galley>,
    parts: Vec<FootPart>,
    height: f32,
}

/// Where every part of the page stands, for one width, theme, canvas height and
/// state of the link.
#[derive(Clone)]
pub(in crate::app) struct FrontDeskMetrics {
    heading: Arc<Galley>,
    lead: Arc<Galley>,
    /// The left edge of the heading, which is centred in the block.
    heading_left: f32,
    lead_top: f32,
    /// The number of columns of steps: three in one row, or one in a column.
    columns: usize,
    /// Whether the four tiles stand under the link.
    open: bool,
    step_left: [f32; 3],
    /// The top of each step, from the top of the block.
    step_top: [f32; 3],
    steps: [StepText; 3],
    link: Arc<Galley>,
    link_left: f32,
    link_top: f32,
    /// The top of the four tiles under the link, from the top of the block.
    tiles_top: f32,
    foot: Foot,
    foot_top: f32,
    /// The height of everything the page draws, its pads excluded.
    pub(in crate::app) block_height: f32,
}

impl FrontDeskMetrics {
    /// Measures the page of `width` for the rows of the shortcuts in `lines`, with
    /// the four tiles under its link when `tiles` names them.
    pub(in crate::app) fn measure(
        painter: &Painter,
        width: f32,
        lines: &[(String, String)],
        tiles: Option<WelcomeMetrics>,
    ) -> Self {
        let heading = painter.layout_job(heading_job(HEADING, HEADING_FONT, theme::FG(), Weight::Black));
        let lead = painter.layout_job(centred_job(
            LEAD,
            &FontId::proportional(LEAD_FONT),
            theme::FG_SOFT(),
            LEAD_MAX_WIDTH.min(width.max(1.0)),
            Weight::Regular,
        ));
        let heading_left = ((width - heading.size().x) * 0.5).max(0.0);
        let lead_top = heading.size().y + HEADING_GAP;
        // The top of the steps, from the top of the block.
        let steps_start = lead_top + lead.size().y + BLOCK_GAP;

        let columns = step_columns(width);
        let step_width = if columns == 1 {
            STEP_WIDTH.min(width)
        } else {
            STEP_WIDTH
        };
        let steps: [StepText; 3] =
            std::array::from_fn(|index| StepText::measure(painter, index, &STEPS[index], step_width));
        let heights: [f32; 3] = std::array::from_fn(|index| steps[index].height());
        let (step_left, tops, steps_height) = step_places(columns, width, &heights);
        let step_top: [f32; 3] = std::array::from_fn(|index| steps_start + tops[index]);

        let open = tiles.is_some();
        let link_text = if open { FEWER } else { MORE };
        let link = painter.layout_job(heading_job(link_text, ACTION_FONT, theme::ACCENT(), Weight::Semibold));
        let link_left = ((width - link.size().x - CHEVRON_GAP - CHEVRON) * 0.5).max(0.0);
        let link_top = steps_start + steps_height + BLOCK_GAP;
        let tiles_top = link_top + link.size().y + TILES_GAP;
        let under_link = match tiles {
            Some(tiles) => tiles_top + tiles.grid_height(),
            None => link_top + link.size().y,
        };
        let foot = Foot::measure(painter, width, lines);
        let foot_height = foot.height;
        let foot_top = under_link + SHORTCUTS_GAP;
        Self {
            heading,
            lead,
            heading_left,
            lead_top,
            columns,
            open,
            step_left,
            step_top,
            steps,
            link,
            link_left,
            link_top,
            tiles_top,
            foot,
            foot_top,
            block_height: foot_top + foot_height,
        }
    }
}

impl StepText {
    /// Lays the text of the step that `index` counts from zero out for `width`.
    fn measure(painter: &Painter, index: usize, step: &Step, width: f32) -> Self {
        let number = (index + 1).to_string();
        let digit = painter.layout_job(heading_job(&number, CHIP_DIGIT_FONT, theme::ACCENT(), Weight::Semibold));
        let title = painter.layout_job(tile_title_job(
            step.title,
            &FontId::proportional(STEP_TITLE_FONT),
            theme::FG(),
            width,
            Weight::Semibold,
        ));
        let mut job = tile_title_job(
            step.sentence,
            &FontId::proportional(STEP_SENTENCE_FONT),
            theme::FG_SOFT(),
            width,
            Weight::Regular,
        );
        job.wrap.max_rows = STEP_SENTENCE_ROWS;
        let sentence = painter.layout_job(job);
        let title_top = CHIP + STEP_TITLE_GAP;
        let sentence_top = title_top + title.size().y + STEP_SENTENCE_GAP;
        Self {
            digit,
            title,
            sentence,
            title_top,
            sentence_top,
        }
    }

    /// The height of the step: its chip, its title and its sentence.
    fn height(&self) -> f32 {
        self.sentence_top + self.sentence.size().y
    }
}

impl Foot {
    /// Lays the row out for a block `width` wide, with the lines of this computer.
    fn measure(painter: &Painter, width: f32, lines: &[(String, String)]) -> Self {
        let lead = painter.layout_no_wrap(
            SHORTCUTS_LEAD.to_owned(),
            FontId::proportional(SHORTCUTS_FONT),
            theme::FG_SOFT(),
        );
        let (height, mut x, mut y) = (lead.size().y, lead.size().x + SHORTCUT_GAP, 0.0_f32);
        let mut parts = Vec::with_capacity(lines.len());
        for (label, keys) in lines {
            let label = painter.layout_no_wrap(label.clone(), FontId::proportional(SHORTCUTS_FONT), theme::FG_SOFT());
            let key = painter.layout_no_wrap(keys.clone(), FontId::monospace(KEY_FONT), theme::FG_SOFT());
            let chip_size = key.size() + Vec2::new(12.0, 4.0);
            // A name and its key that do not fit the width of the page start the
            // next row, so a narrow window cuts no part of the line at its right.
            if x > 0.0 && x + label.size().x + 6.0 + chip_size.x > width {
                x = 0.0;
                y += SHORTCUT_ROW;
            }
            let label_pos = Pos2::new(x, y);
            let chip = Rect::from_min_size(
                Pos2::new(x + label.size().x + 6.0, y + (height - key.size().y) * 0.5 - 1.0),
                chip_size,
            );
            x = chip.max.x + SHORTCUT_GAP;
            parts.push(FootPart {
                label,
                key,
                label_pos,
                chip,
            });
        }
        Self { lead, parts, height }
    }

    /// Paints the row at `origin`, the left edge of the block and the top of the
    /// row.
    fn paint(&self, painter: &Painter, origin: Pos2) {
        painter.galley(origin, self.lead.clone(), theme::FG_SOFT());
        for part in &self.parts {
            painter.galley(origin + part.label_pos.to_vec2(), part.label.clone(), theme::FG_SOFT());
            let chip = part.chip.translate(origin.to_vec2());
            painter.rect_filled(chip, CornerRadius::same(KEY_RADIUS), theme::PANEL_BG());
            painter.rect_stroke(
                chip,
                CornerRadius::same(KEY_RADIUS),
                Stroke::new(1.0, theme::BORDER_SUBTLE()),
                StrokeKind::Inside,
            );
            painter.galley(chip.min + Vec2::new(6.0, 2.0), part.key.clone(), theme::FG_SOFT());
        }
    }
}

/// The number of columns of steps that a block `width` wide holds: three in one
/// row, or one in a column.
fn step_columns(width: f32) -> usize {
    let row = 3.0 * STEP_WIDTH + 2.0 * STEP_GAP + 2.0 * STEP_ROW_PAD;
    if width.is_finite() && width >= row { 3 } else { 1 }
}

/// Where the steps stand in a block, from its left edge and its top, and the
/// height of the whole group.
fn step_places(columns: usize, width: f32, heights: &[f32; 3]) -> ([f32; 3], [f32; 3], f32) {
    if columns == 1 {
        let left = [0.0; 3];
        let top = [
            0.0,
            heights[0] + STEP_COLUMN_GAP,
            heights[0] + heights[1] + 2.0 * STEP_COLUMN_GAP,
        ];
        let height = heights[0] + heights[1] + heights[2] + 2.0 * STEP_COLUMN_GAP;
        return (left, top, height);
    }
    let group = 3.0 * STEP_WIDTH + 2.0 * STEP_GAP;
    let first = ((width - group) * 0.5).max(0.0);
    let left = std::array::from_fn(|index| first + usize_to_f32(index) * (STEP_WIDTH + STEP_GAP));
    (left, [0.0; 3], heights[0].max(heights[1]).max(heights[2]))
}

/// The distance from the top of the canvas to the top of the block of the page.
///
/// The middle of the block stands at [`BLOCK_CENTRE`] of the height of the
/// canvas. A block that is taller than the canvas keeps the top pad of the page
/// and scrolls under it.
pub(in crate::app) fn block_top(canvas_height: f32, block_height: f32) -> f32 {
    let centred = canvas_height * BLOCK_CENTRE - block_height * 0.5;
    centred.max(f32::from(PAGE_PAD_TOP))
}

/// What a frame of the page pressed.
#[derive(Default)]
pub(in crate::app) struct FrontDeskPress {
    /// Whether the person pressed the link that turns the four tiles over.
    pub(in crate::app) link: bool,
    /// The tile that the person pressed under the link.
    pub(in crate::app) tile: Option<WelcomeTile>,
}

/// Paints the page inside `content` and answers the presses of the person.
fn paint(ui: &mut Ui, content: Rect, metrics: &FrontDeskMetrics, tiles: Option<WelcomeMetrics>) -> FrontDeskPress {
    let painter = ui.painter().clone();
    let origin = content.min;
    painter.galley(
        origin + Vec2::new(metrics.heading_left, 0.0),
        metrics.heading.clone(),
        theme::FG(),
    );
    // The rows of a centred job stand around its own origin, so the line under the
    // heading is painted on the middle line of the block.
    painter.galley(
        origin + Vec2::new(content.width() * 0.5, metrics.lead_top),
        metrics.lead.clone(),
        theme::FG_SOFT(),
    );
    paint_steps(&painter, origin, metrics);
    let link = paint_link(ui, origin, metrics);
    let tile = tiles.and_then(|tiles| paint_tiles(ui, origin, metrics, tiles));
    metrics.foot.paint(&painter, origin + Vec2::new(0.0, metrics.foot_top));
    FrontDeskPress { link, tile }
}

/// Paints the three steps, and the lines that join their chips.
fn paint_steps(painter: &Painter, origin: Pos2, metrics: &FrontDeskMetrics) {
    for (index, step) in metrics.steps.iter().enumerate() {
        let left = origin.x + metrics.step_left[index];
        let top = origin.y + metrics.step_top[index];
        let chip = Rect::from_min_size(Pos2::new(left, top), Vec2::splat(CHIP));
        painter.rect_stroke(
            chip,
            CornerRadius::same(CHIP_RADIUS),
            Stroke::new(1.0, theme::alpha(theme::ACCENT(), 140)),
            StrokeKind::Inside,
        );
        let digit = step.digit.size();
        painter.galley(chip.center() - digit * 0.5, step.digit.clone(), theme::ACCENT());
        painter.galley(Pos2::new(left, top + step.title_top), step.title.clone(), theme::FG());
        painter.galley(
            Pos2::new(left, top + step.sentence_top),
            step.sentence.clone(),
            theme::FG_SOFT(),
        );
    }
    if metrics.columns == 3 {
        let ink = Stroke::new(1.0, theme::BORDER_SUBTLE());
        for index in 0..2 {
            let y = origin.y + metrics.step_top[index] + CHIP * 0.5;
            let from = origin.x + metrics.step_left[index] + CHIP + JOIN_INSET;
            let to = origin.x + metrics.step_left[index + 1] - JOIN_INSET;
            painter.line_segment([Pos2::new(from, y), Pos2::new(to, y)], ink);
        }
    }
}

/// Paints the link under the steps, and answers whether the person pressed it.
fn paint_link(ui: &mut Ui, origin: Pos2, metrics: &FrontDeskMetrics) -> bool {
    let left = origin.x + metrics.link_left;
    let top = origin.y + metrics.link_top;
    let size = metrics.link.size();
    let width = size.x + CHEVRON_GAP + CHEVRON;
    let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, size.y));
    let response = ui.interact(rect, ui.make_persistent_id("front_desk_more_link"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, if metrics.open { FEWER } else { MORE }));
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    painter.galley(Pos2::new(left, top), metrics.link.clone(), theme::ACCENT());
    let ink = Stroke::new(1.6, theme::ACCENT());
    let center = top + size.y * 0.5;
    let start = left + size.x + CHEVRON_GAP;
    painter.line_segment(
        [Pos2::new(start, center - CHEVRON), Pos2::new(start + CHEVRON, center)],
        ink,
    );
    painter.line_segment(
        [Pos2::new(start + CHEVRON, center), Pos2::new(start, center + CHEVRON)],
        ink,
    );
    response.clicked()
}

/// Paints the four tiles under the link, and answers the tile that the person
/// pressed.
fn paint_tiles(ui: &mut Ui, origin: Pos2, metrics: &FrontDeskMetrics, tiles: WelcomeMetrics) -> Option<WelcomeTile> {
    let mut pressed = None;
    for (index, tile) in WelcomeTile::ALL.iter().enumerate() {
        let (row, column) = tiles.cell(index);
        let x = origin.x + usize_to_f32(column) * (tiles.tile_width + TILE_GAP);
        let top = origin.y + metrics.tiles_top + usize_to_f32(row) * (tiles.tile_height + ROW_GAP);
        let rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(tiles.tile_width, tiles.tile_height));
        if super::paint_tile(ui, *tile, rect, index, tiles) {
            pressed = Some(*tile);
        }
    }
    pressed
}

impl super::HorizonApp {
    /// Draws the first start page of a window that shows the orchestrator dock.
    pub(in crate::app) fn render_front_desk_page(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let width = page_width(canvas.width());
        let measure = ctx.layer_painter(LayerId::new(Order::Background, Id::unique("front_desk_page_measure")));
        let tiles = self
            .welcome
            .link_open()
            .then(|| self.welcome.metrics(&measure, width, 0, 0, canvas.height()));
        let lines = self.shortcut_lines();
        let metrics = self.welcome.front_desk(&measure, width, &lines, tiles);
        let top = canvas.min.y + block_top(canvas.height(), metrics.block_height);
        let left = canvas.min.x + (canvas.width() - width) * 0.5;
        let inner = Rect::from_min_max(Pos2::new(left, top), Pos2::new(left + width, canvas.max.y));
        let pressed = page_frame(
            ctx,
            inner,
            metrics.block_height + PAGE_PAD_BOTTOM,
            "front_desk_page",
            |ui, content| paint(ui, content, &metrics, tiles),
        )
        .unwrap_or_default();
        if pressed.link {
            self.welcome.toggle_link();
        }
        if let Some(tile) = pressed.tile {
            self.start_welcome_tile(ctx, tile);
        }
    }
}
