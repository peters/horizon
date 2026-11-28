//! The empty state of the dock: what the conversation does, centred between the
//! header and the composer, with two example rows under the two sentences.
//!
//! The dock shows one of two conversations, so this module has two forms. A
//! workspace names the orchestrator of that workspace; the front desk names its
//! own job, and its example rows ask for work items. Both forms keep the same
//! sizes and the same gaps.
//!
//! A row of an example is a text that the person can put into the draft with one
//! click. This module draws the rows and answers the text of the row that the
//! person clicked; the dock keeps the draft of that conversation and sends
//! nothing.

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
/// The empty state of the front desk: the question that it asks the person, what
/// it does with the message, and the two example rows of a conversation that
/// starts work items.
pub(super) const FRONT_DESK_TITLE: &str = "What should be done?";
pub(super) const FRONT_DESK_SENTENCES: &str = "Write one message with one task or more. The front desk \
                                                starts a work item for each task and tells you when one \
                                                needs you.";
const FRONT_DESK_EXAMPLES: [&str; 2] = ["Fix #1987 and make the cold start faster", "What waits for me?"];
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

/// What the empty state of the dock describes: the orchestrator of a workspace,
/// or the front desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Form {
    Workspace,
    FrontDesk,
}

impl Form {
    /// The empty state of the conversation that the dock shows, which the dock
    /// names as one value.
    pub(super) const fn of(shows_front_desk: bool) -> Self {
        if shows_front_desk {
            Self::FrontDesk
        } else {
            Self::Workspace
        }
    }

    /// The title of this form.
    fn title(self) -> &'static str {
        match self {
            Self::Workspace => TITLE,
            Self::FrontDesk => FRONT_DESK_TITLE,
        }
    }

    /// The two sentences under the title of this form.
    fn sentences(self) -> &'static str {
        match self {
            Self::Workspace => SENTENCES,
            Self::FrontDesk => FRONT_DESK_SENTENCES,
        }
    }

    /// The two example rows of this form.
    fn examples(self) -> [&'static str; 2] {
        match self {
            Self::Workspace => EXAMPLES,
            Self::FrontDesk => FRONT_DESK_EXAMPLES,
        }
    }
}

/// Draws the empty state of `form` in `body`, and answers the text of the example
/// that the person clicked.
pub(super) fn render(ui: &Ui, body: Rect, form: Form) -> Option<&'static str> {
    let text_width = body.width() - 2.0 * PADDING_X;
    if text_width <= 0.0 || body.height() <= 0.0 {
        return None;
    }
    let examples_of = form.examples();
    let title = text::line(ui, form.title(), TITLE_SIZE, Weight::Bold, theme::FG(), text_width);
    let sentences = text::paragraph(
        ui,
        form.sentences(),
        SENTENCES_SIZE,
        Weight::Regular,
        theme::FG_SOFT(),
        text_width,
        SENTENCES_LINE_HEIGHT,
    );
    let examples: Vec<Arc<Galley>> = examples_of
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
            picked = Some(examples_of[index]);
        }
        row_top += example_height + EXAMPLE_ROW_GAP;
    }
    picked
}
