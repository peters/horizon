//! The state mark of speech input: a small pill on the panel of a take.
//!
//! The pill reports one state of one take and nothing else. It is painted, not
//! added as a widget, so a press on the panel body still reaches the terminal
//! under it, and it is drawn once per frame while a take records or while a
//! sentence of an error waits.

use egui::{CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::text::single_line_label_job;
use crate::theme;

use super::tone_color;

/// The size of the text of the mark.
const MARK_FONT: f32 = 11.5;

/// The height of the pill.
const MARK_HEIGHT: f32 = 20.0;

/// The space between the text and the edge of the pill.
const MARK_PAD: f32 = 8.0;

/// The space between the dot and the text.
const MARK_DOT_GAP: f32 = 6.0;

/// The diameter of the dot of a state.
const MARK_DOT: f32 = 6.0;

/// The distance of the pill from the edges of the panel.
const MARK_MARGIN: f32 = 10.0;

/// The tone of one line of the mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum MarkTone {
    /// The recorder of the take runs.
    Listening,
    /// The take is transcribed.
    Writing,
    /// The take failed, and the mark says why.
    Error,
}

/// The rectangle of the pill of a text of `text_size`, at the bottom left of
/// `panel`, inside the body of the panel.
pub(super) fn mark_rect(panel: Rect, text_size: Vec2) -> Rect {
    let width = 2.0 * MARK_PAD + MARK_DOT + MARK_DOT_GAP + text_size.x;
    let origin = Pos2::new(panel.min.x + MARK_MARGIN, panel.max.y - MARK_MARGIN - MARK_HEIGHT);
    Rect::from_min_size(origin, Vec2::new(width, MARK_HEIGHT))
}

/// Paint the mark `text` at the bottom left of `panel`, inside the body of the
/// panel.
pub(super) fn paint(painter: &Painter, panel: Rect, text: &str, tone: MarkTone) {
    let color = tone_color(tone);
    let font = FontId::monospace(MARK_FONT);
    let galley = painter.layout_job(single_line_label_job(text, &font, color, 420.0));
    let rect = mark_rect(panel, galley.size());
    let radius = CornerRadius::same(10);
    painter.rect_filled(rect, radius, theme::BG_ELEVATED());
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), color, 0.4)),
        StrokeKind::Inside,
    );
    let center = rect.center();
    let dot = Pos2::new(rect.min.x + MARK_PAD + MARK_DOT * 0.5, center.y);
    match tone {
        MarkTone::Listening | MarkTone::Error => {
            painter.circle_filled(dot, MARK_DOT * 0.5, color);
        }
        MarkTone::Writing => {
            painter.circle_stroke(dot, MARK_DOT * 0.5 - 0.75, Stroke::new(1.5, color));
        }
    }
    let text_origin = Pos2::new(
        rect.min.x + MARK_PAD + MARK_DOT + MARK_DOT_GAP,
        center.y - galley.size().y * 0.5,
    );
    painter.galley(text_origin, galley, color);
}
