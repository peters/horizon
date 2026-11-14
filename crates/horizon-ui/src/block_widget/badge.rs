//! The badge: a rounded pill of one line, with a dot for a state tone.
//!
//! A badge is drawn on the row of a heading, or on a row of its own. The host
//! may put a fixed prefix in front of its text, which is how a card whose agent
//! is gone keeps the agent's last words and marks them as old.

use egui::{CornerRadius, FontId, Id, Painter, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{Badge, Tone};

use crate::theme;

use super::metrics::{BADGE_DOT, BADGE_DOT_GAP, BADGE_FONT, BADGE_HEIGHT, BADGE_PAD_SIDE, RADIUS_CONTROL};
use super::text;
use super::tones;

/// The prefix the host puts in front of every badge while the owner is gone.
pub(super) const LAST_SEEN: &str = "Last seen: ";

/// Width one pill needs for `text`, including its padding and its dot.
pub(super) fn width(ui: &Ui, text: &str, prefix: Option<&str>, tone: Tone) -> f32 {
    let font = FontId::proportional(BADGE_FONT);
    let mut width = 2.0 * BADGE_PAD_SIDE
        + text::line(ui.painter(), text, &font, theme::FG(), f32::INFINITY)
            .size()
            .x;
    if let Some(prefix) = prefix {
        width += text::line(ui.painter(), prefix, &font, theme::FG(), f32::INFINITY)
            .size()
            .x;
    }
    if tones::badge_has_dot(tone) {
        width += BADGE_DOT + BADGE_DOT_GAP;
    }
    width
}

/// Paint one pill inside `rect`, which the caller sizes and places.
pub(super) fn paint(painter: &Painter, rect: Rect, text: &str, prefix: Option<&str>, tone: Tone) {
    let colours = tones::badge(tone);
    let radius = CornerRadius::same(RADIUS_CONTROL);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, colours.outline), StrokeKind::Inside);

    let font = FontId::proportional(BADGE_FONT);
    let center_y = rect.center().y;
    let mut x = rect.min.x + BADGE_PAD_SIDE;
    if tones::badge_has_dot(tone) {
        painter.circle_filled(Pos2::new(x + BADGE_DOT * 0.5, center_y), BADGE_DOT * 0.5, colours.text);
        x += BADGE_DOT + BADGE_DOT_GAP;
    }
    for part in [prefix, Some(text)].into_iter().flatten() {
        let galley = text::line(painter, part, &font, colours.text, f32::INFINITY);
        let height = galley.size().y;
        let part_width = galley.size().x;
        painter.galley(Pos2::new(x, center_y - height * 0.5), galley, colours.text);
        x += part_width;
    }
}

/// Registers one pill as a single node, with the prefix the host put in front
/// of it, so a reader hears the badge the way it is drawn.
pub(super) fn node(ui: &Ui, id: Id, rect: Rect, text: &str, prefix: Option<&str>) {
    let response = ui.interact(rect, id, Sense::hover());
    match prefix {
        None => response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text)),
        Some(prefix) => {
            let spoken = format!("{prefix}{text}");
            response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &spoken));
        }
    }
}

/// The pill of a heading, right aligned on the heading row: its width, the tone
/// the host gives it and the prefix the host adds.
pub(super) fn heading_pill(ui: &Ui, badge: &Badge, agent_present: bool) -> (f32, Tone, Option<&'static str>) {
    let (tone, prefix) = host_tone(badge.tone, agent_present);
    (width(ui, &badge.text, prefix, tone), tone, prefix)
}

/// Draw one `badge` component: a pill on the left of a row of its own.
pub(super) fn show(ui: &mut Ui, index: usize, text: &str, tone: Tone, agent_present: bool) {
    let (tone, prefix) = host_tone(tone, agent_present);
    let pill = width(ui, text, prefix, tone);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), BADGE_HEIGHT), Sense::hover());
    let pill_rect = Rect::from_min_size(rect.min, Vec2::new(pill.min(rect.width()), BADGE_HEIGHT));
    paint(ui.painter(), pill_rect, text, prefix, tone);
    node(
        ui,
        ui.make_persistent_id(("block_badge", index)),
        pill_rect,
        text,
        prefix,
    );
}

/// The tone and the prefix the host gives a badge: while the owner is gone
/// every badge is neutral and says when it was last seen.
fn host_tone(tone: Tone, agent_present: bool) -> (Tone, Option<&'static str>) {
    if agent_present {
        (tone, None)
    } else {
        (Tone::Neutral, Some(LAST_SEEN))
    }
}
