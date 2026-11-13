//! Text layout for the painted parts of a card.
//!
//! Every component of a card is painted, not added as a stock widget: the
//! mockups need fills, outlines, marks and truncation that a stock label
//! cannot carry. Layout still goes through egui's font atlas and galley cache,
//! so a text is shaped once and reused while it and its font stay the same.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, Painter};

use crate::text::single_line_label_job;
use crate::typeface::{self, Weight};
use crate::zoom_text;

/// One line of `text`, elided with an ellipsis when it is wider than
/// `max_width`.
pub(super) fn line(painter: &Painter, text: &str, font: &FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    line_weight(painter, text, font, color, max_width, Weight::Regular)
}

/// [`line`] at `weight`.
pub(super) fn line_weight(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
    weight: Weight,
) -> Arc<Galley> {
    let mut job = single_line_label_job(text, font, color, max_width);
    for section in &mut job.sections {
        section.format.coords = weight.coords();
    }
    zoom_text::layout_job(painter, job)
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
    wrapped_weight(painter, text, font, color, max_width, line_height, Weight::Regular)
}

/// [`wrapped`] at `weight`.
pub(super) fn wrapped_weight(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
    line_height: Option<f32>,
    weight: Weight,
) -> Arc<Galley> {
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: max_width.max(0.0),
            ..TextWrapping::default()
        },
        ..LayoutJob::default()
    };
    job.append(text, 0.0, {
        let mut format = typeface::format(font, color, weight);
        format.line_height = line_height;
        format
    });
    zoom_text::layout_job(painter, job)
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
    weight: Weight,
) -> Arc<Galley> {
    let mut job = single_line_label_job(&text.to_uppercase(), font, color, max_width);
    for section in &mut job.sections {
        section.format.extra_letter_spacing = tracking;
        section.format.coords = weight.coords();
    }
    zoom_text::layout_job(painter, job)
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Context, FontId};

    use super::{line, wrapped};
    use crate::test_egui::with_canvas_layer;

    /// The size of the glyphs that a galley of `text` asks the rasteriser for.
    fn glyph_size(galley: &egui::Galley) -> f32 {
        galley.job.sections[0].format.font_id.size
    }

    /// One line of card text in a fresh context, so that the frame of the layer
    /// is the first frame that its state sees.
    fn one_line(zoom: f32) -> std::sync::Arc<egui::Galley> {
        let ctx = Context::default();
        let font = FontId::proportional(18.0);
        with_canvas_layer(&ctx, zoom, |ui| {
            line(
                ui.painter(),
                "build-1 and the machine it runs on",
                &font,
                Color32::WHITE,
                400.0,
            )
        })
        .expect("one frame")
    }

    #[test]
    fn the_text_of_a_card_is_made_for_the_zoom_it_is_drawn_at() {
        let plain = one_line(1.0);
        let zoomed = one_line(0.67);

        assert!(
            (glyph_size(&plain) - 18.0).abs() < f32::EPSILON,
            "zoom 1 keeps the size of the card"
        );
        assert!(
            (glyph_size(&zoomed) - 12.0).abs() < f32::EPSILON,
            "0.67 of 18 points is 12.06, on the ladder 12.0, not {}",
            glyph_size(&zoomed)
        );
        // The layout does not jump with the zoom: the line keeps the width and
        // the height of zoom 1, because only the glyph pictures were made anew.
        assert!(
            (plain.size().x - zoomed.size().x).abs() <= 0.01 * plain.size().x,
            "{} against {}",
            plain.size().x,
            zoomed.size().x
        );
        assert!(
            (plain.size().y - zoomed.size().y).abs() <= 0.02 * plain.size().y,
            "the line keeps the row height of zoom 1: {} against {}",
            plain.size().y,
            zoomed.size().y
        );
    }

    #[test]
    fn a_wrapped_paragraph_of_a_card_keeps_its_words_at_every_zoom() {
        let font = FontId::proportional(12.5);
        let text = "The machine was created at the provider and the agent is waiting for it.";
        let rows = |zoom: f32| {
            let ctx = Context::default();
            with_canvas_layer(&ctx, zoom, |ui| {
                wrapped(ui.painter(), text, &font, Color32::WHITE, 150.0, None)
            })
            .expect("one frame")
            .rows
            .iter()
            .map(|placed| placed.row.glyphs.iter().map(|glyph| glyph.chr).collect::<String>())
            .collect::<Vec<String>>()
        };

        let plain = rows(1.0);
        assert!(plain.len() > 1, "the paragraph should wrap: {plain:?}");
        assert_eq!(rows(0.67), plain);
        assert_eq!(rows(1.5), plain);
    }
}
