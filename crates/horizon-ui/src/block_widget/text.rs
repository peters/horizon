//! Text layout for the painted parts of a card.
//!
//! Every component of a card is painted, not added as a stock widget: the
//! mockups need fills, outlines, marks and truncation that a stock label
//! cannot carry. Layout still goes through egui's font atlas and galley cache,
//! so a text is shaped once and reused while it and its font stay the same.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Galley, Painter};

use crate::text::single_line_label_job;

/// One line of `text`, elided with an ellipsis when it is wider than
/// `max_width`.
pub(super) fn line(painter: &Painter, text: &str, font: &FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let job = single_line_label_job(text, font, color, max_width);
    painter.layout_job(job)
}

/// `text` wrapped to `max_width` at `line_height`, which is `None` for the
/// font's own line height.
pub(super) fn wrapped(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
    line_height: Option<f32>,
) -> Arc<Galley> {
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: max_width.max(0.0),
            ..TextWrapping::default()
        },
        ..LayoutJob::default()
    };
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            line_height,
            ..TextFormat::default()
        },
    );
    painter.layout_job(job)
}

/// One line of `text` in upper case, with `tracking` between its letters, as a
/// caption draws it.
pub(super) fn spaced_line(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
    tracking: f32,
) -> Arc<Galley> {
    let mut job = single_line_label_job(&text.to_uppercase(), font, color, max_width);
    for section in &mut job.sections {
        section.format.extra_letter_spacing = tracking;
    }
    painter.layout_job(job)
}
