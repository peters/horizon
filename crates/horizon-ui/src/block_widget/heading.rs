//! The heading of a card: one strong line that takes the lines its words need,
//! an optional second line and an optional badge that shares the heading row or
//! takes one of its own.

use egui::{FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::Badge;

use crate::theme;
use crate::typeface::Weight;

use super::badge;
use super::metrics::{
    BADGE_HEIGHT, HEADING_BADGE_GAP, HEADING_BADGE_ROW_GAP, HEADING_FONT, HEADING_SUBTITLE_FONT, HEADING_SUBTITLE_GAP,
    HEADING_SUBTITLE_LINE_HEIGHT, WIDE_HEADING_FONT, WIDE_PROGRESS_GAP, WIDE_PROGRESS_WIDTH,
};
use super::text;

/// The least share of the row that the heading text keeps beside a badge.
const HEADING_MIN_SHARE: f32 = 0.6;

/// Where the badge of a heading is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BadgeRow {
    /// At the right end of the heading row.
    SameRow,
    /// On a row of its own, under the second line, left aligned.
    OwnRow,
}

/// Which row the badge of a heading takes, from the room the row has and the
/// width the heading and the badge ask for.
///
/// The heading names the card, so it keeps at least [`HEADING_MIN_SHARE`] of the
/// row. A badge that would take more than the rest gives up the row and stands
/// under the second line instead, so a narrow card cuts neither of them.
pub(super) fn badge_row(row_width: f32, heading_width: f32, badge_width: f32) -> BadgeRow {
    let both_fit = heading_width + HEADING_BADGE_GAP + badge_width <= row_width;
    if both_fit || text_room(row_width, badge_width) >= HEADING_MIN_SHARE * row_width {
        BadgeRow::SameRow
    } else {
        BadgeRow::OwnRow
    }
}

/// The room the heading text keeps while a badge of `badge_width` shares its row.
fn text_room(row_width: f32, badge_width: f32) -> f32 {
    (row_width - badge_width - HEADING_BADGE_GAP).max(0.0)
}

/// The parts of a heading, which both forms draw.
#[derive(Clone, Copy)]
pub(super) struct Parts<'a> {
    /// Title text.
    pub(super) text: &'a str,
    /// Optional second line under the title.
    pub(super) subtitle: Option<&'a str>,
    /// Optional badge on the title row, right aligned.
    pub(super) badge: Option<&'a Badge>,
}

/// Draw the heading of component `index` and return the row it took.
pub(super) fn show(ui: &mut Ui, index: usize, parts: Parts<'_>, agent_present: bool) -> Rect {
    show_row(ui, index, parts, HEADING_FONT, 0.0, 0.0, agent_present)
}

/// Draw the heading of a wide card that a progress bar stands beside, at the
/// larger size of that row, with the room of the bar kept free at the right, and
/// in a row at least `least_height` tall.
pub(super) fn show_wide(ui: &mut Ui, index: usize, parts: Parts<'_>, agent_present: bool, least_height: f32) -> Rect {
    show_row(
        ui,
        index,
        parts,
        WIDE_HEADING_FONT,
        WIDE_PROGRESS_WIDTH + WIDE_PROGRESS_GAP,
        least_height,
        agent_present,
    )
}

/// Draw a heading at `font_size`, keeping `reserve` points free at the right of
/// the row for the part that stands beside it, in a row at least `least_height`
/// tall.
fn show_row(
    ui: &mut Ui,
    index: usize,
    parts: Parts<'_>,
    font_size: f32,
    reserve: f32,
    least_height: f32,
    agent_present: bool,
) -> Rect {
    let Parts { text, subtitle, badge } = parts;
    let width = ui.available_width();
    let room = (width - reserve).max(0.0);
    let pill = badge.map(|badge| badge::heading_pill(ui, badge, agent_present));
    let pill_width = pill.map_or(0.0, |(width, _, _)| width).min(room).max(0.0);
    let font = FontId::proportional(font_size);
    let heading_width = text::line(ui.painter(), text, &font, theme::FG(), f32::INFINITY)
        .size()
        .x;
    let row = match pill {
        Some(_) => badge_row(room, heading_width, pill_width),
        None => BadgeRow::SameRow,
    };
    let text_width = match (row, pill.is_some()) {
        (BadgeRow::SameRow, true) => text_room(room, pill_width),
        _ => room,
    };

    let title = text::wrapped_weight(ui.painter(), text, &font, theme::FG(), text_width, None, Weight::Bold);
    let title_height = title.size().y;
    // The second line says what the card is for, so it takes every line that
    // its words need inside the room that the card has, and the height of the
    // heading counts them.
    let subtitle_galley = subtitle.map(|subtitle| {
        text::wrapped(
            ui.painter(),
            subtitle,
            &FontId::proportional(HEADING_SUBTITLE_FONT),
            theme::FG_SOFT(),
            text_width,
            Some(HEADING_SUBTITLE_FONT * HEADING_SUBTITLE_LINE_HEIGHT),
        )
    });
    let subtitle_height = subtitle_galley.as_ref().map_or(0.0, |galley| galley.size().y);
    let subtitle_span = if subtitle.is_some() {
        HEADING_SUBTITLE_GAP + subtitle_height
    } else {
        0.0
    };
    let mut height = title_height + subtitle_span;
    if pill.is_some() {
        height = match row {
            BadgeRow::SameRow => height.max(BADGE_HEIGHT),
            BadgeRow::OwnRow => height + HEADING_BADGE_ROW_GAP + BADGE_HEIGHT,
        };
    }
    // The part that stands beside the heading may be taller than the heading,
    // and the row takes the height of the taller of the two.
    let height = height.max(least_height);

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));

    let painter = ui.painter();
    painter.galley(rect.min, title, theme::FG());
    if let Some(galley) = subtitle_galley {
        let top = rect.min.y + title_height + HEADING_SUBTITLE_GAP;
        painter.galley(Pos2::new(rect.min.x, top), galley, theme::FG_SOFT());
        label_node(ui, index, rect, top, subtitle);
    }
    if let (Some(badge), Some((pill_width, tone, prefix))) = (badge, pill) {
        let left = match row {
            BadgeRow::SameRow => rect.min.x + room - pill_width,
            BadgeRow::OwnRow => rect.min.x,
        };
        let top = match row {
            BadgeRow::SameRow => rect.min.y,
            BadgeRow::OwnRow => rect.min.y + title_height + subtitle_span + HEADING_BADGE_ROW_GAP,
        };
        let pill_rect = Rect::from_min_size(Pos2::new(left, top), egui::vec2(pill_width, BADGE_HEIGHT));
        badge::paint(painter, pill_rect, &badge.text, prefix, tone);
        badge::node(
            ui,
            ui.make_persistent_id(("block_heading_badge", index)),
            pill_rect,
            &badge.text,
            prefix,
        );
    }
    rect
}

/// Registers the second line as its own node, so a reader hears both lines and
/// a test can find the text that was painted.
fn label_node(ui: &Ui, index: usize, row: Rect, top: f32, subtitle: Option<&str>) {
    let Some(subtitle) = subtitle else { return };
    let rect = Rect::from_min_max(
        Pos2::new(row.min.x, top),
        Pos2::new(row.max.x, top + HEADING_SUBTITLE_FONT * HEADING_SUBTITLE_LINE_HEIGHT),
    );
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_heading_subtitle", index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, subtitle));
}
