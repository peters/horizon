//! The heading of a card: one strong line that takes the lines its words need,
//! an optional second line and an optional badge that shares the heading row or
//! takes one of its own.

use egui::{FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::Badge;

use crate::theme;

use super::badge;
use super::metrics::{
    BADGE_HEIGHT, HEADING_BADGE_GAP, HEADING_BADGE_ROW_GAP, HEADING_FONT, HEADING_SUBTITLE_FONT, HEADING_SUBTITLE_GAP,
    HEADING_SUBTITLE_LINE_HEIGHT,
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

/// Draw the heading of component `index` and return the height it took.
pub(super) fn show(
    ui: &mut Ui,
    index: usize,
    text: &str,
    subtitle: Option<&str>,
    badge: Option<&Badge>,
    agent_present: bool,
) -> f32 {
    let width = ui.available_width();
    let pill = badge.map(|badge| badge::heading_pill(ui, badge, agent_present));
    let pill_width = pill.map_or(0.0, |(width, _, _)| width).min(width).max(0.0);
    let font = FontId::proportional(HEADING_FONT);
    let heading_width = text::line(ui.painter(), text, &font, theme::FG(), f32::INFINITY)
        .size()
        .x;
    let row = match pill {
        Some(_) => badge_row(width, heading_width, pill_width),
        None => BadgeRow::SameRow,
    };
    let text_width = match (row, pill.is_some()) {
        (BadgeRow::SameRow, true) => text_room(width, pill_width),
        _ => width,
    };

    let title = text::wrapped(ui.painter(), text, &font, theme::FG(), text_width, None);
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
            BadgeRow::SameRow => rect.max.x - pill_width,
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
    height
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
