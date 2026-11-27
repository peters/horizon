//! The empty state of the dock: how the orchestrator works, centred between the
//! header and the composer, with two example rows under the two sentences.
//!
//! A row of an example is a text that the person can put into the draft with one
//! click. This module draws the rows and answers the text of the row that the
//! person clicked; the dock keeps the draft of the workspace and sends nothing.

use std::sync::Arc;

use egui::{Galley, Rect, Stroke, Ui, pos2};

use crate::theme;
use crate::typeface::Weight;

use super::text;

/// The empty state of the dock: the least horizontal padding, the title and the
/// two sentences under it.
const PADDING_X: f32 = 28.0;
const TITLE: &str = "Say what you want done.";
const TITLE_SIZE: f32 = 15.0;
const SENTENCES: &str = "The orchestrator reads the repository, makes a plan and shows it as cards. \
                          It asks before a machine is rented.";
const SENTENCES_SIZE: f32 = 13.5;
const SENTENCES_LINE_HEIGHT: f32 = 1.55;
const GAP: f32 = 8.0;
/// The two example rows under the sentences of the empty state: their texts, the
/// air between the sentences and the first row, the air between the two rows,
/// their padding, their radius and their text size.
const EXAMPLES: [&str; 2] = [
    "Fix #1987 on a rented machine",
    "Why is the cold start slow? Measure it first.",
];
const EXAMPLE_GAP: f32 = 12.0;
const EXAMPLE_ROW_GAP: f32 = 8.0;
const EXAMPLE_PADDING_X: f32 = 12.0;
const EXAMPLE_PADDING_Y: f32 = 9.0;
const EXAMPLE_RADIUS: u8 = 10;
const EXAMPLE_SIZE: f32 = 12.5;

/// Draws the empty state in `body`, and answers the text of the example that the
/// person clicked.
pub(super) fn render(ui: &Ui, body: Rect) -> Option<&'static str> {
    let text_width = body.width() - 2.0 * PADDING_X;
    if text_width <= 0.0 || body.height() <= 0.0 {
        return None;
    }
    let title = text::line(ui, TITLE, TITLE_SIZE, Weight::Bold, theme::FG(), text_width);
    let sentences = text::paragraph(
        ui,
        SENTENCES,
        SENTENCES_SIZE,
        Weight::Regular,
        theme::FG_SOFT(),
        text_width,
        SENTENCES_LINE_HEIGHT,
    );
    let examples: Vec<Arc<Galley>> = EXAMPLES
        .iter()
        .map(|text| {
            text::line(
                ui,
                text,
                EXAMPLE_SIZE,
                Weight::Regular,
                theme::FG(),
                (text_width - 2.0 * EXAMPLE_PADDING_X).max(1.0),
            )
        })
        .collect();
    let example_height = examples
        .first()
        .map_or(0.0, |example| example.size().y + 2.0 * EXAMPLE_PADDING_Y);
    let title_height = title.size().y;
    let height = title_height + GAP + sentences.size().y + EXAMPLE_GAP + 2.0 * example_height + EXAMPLE_ROW_GAP;
    let left = body.min.x + PADDING_X;
    let top = body.center().y - height / 2.0;
    if top < body.min.y || top + height > body.max.y {
        return None;
    }
    let painter = ui.painter();
    painter.galley(pos2(left, top), title, theme::FG());
    let sentences_top = top + title_height + GAP;
    painter.galley(pos2(left, sentences_top), Arc::clone(&sentences), theme::FG_SOFT());
    let mut row_top = sentences_top + sentences.size().y + EXAMPLE_GAP;
    let mut picked = None;
    for (index, example) in examples.into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left, row_top), egui::vec2(text_width, example_height));
        let response = ui.interact(
            rect,
            egui::Id::unique(("orchestrator_dock_example", index)),
            egui::Sense::click(),
        );
        let outline = if response.hovered() {
            theme::FG_DIM()
        } else {
            theme::BORDER_SUBTLE()
        };
        painter.rect(
            rect,
            EXAMPLE_RADIUS,
            theme::PANEL_BG(),
            Stroke::new(1.0_f32, outline),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            pos2(left + EXAMPLE_PADDING_X, row_top + EXAMPLE_PADDING_Y),
            example,
            theme::FG(),
        );
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            picked = Some(EXAMPLES[index]);
        }
        row_top += example_height + EXAMPLE_ROW_GAP;
    }
    picked
}
