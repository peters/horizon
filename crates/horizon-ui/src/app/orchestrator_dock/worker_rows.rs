//! The rows of the workers of an orchestrator in the list of the dock, and the row
//! of one request for a worker that the person decides.
//!
//! The conversation of an orchestrator gets one row for each start of a worker
//! (the reference row of that panel), one bubble for each message that the
//! orchestrator sent to a worker, and one text for each report of a worker. A
//! request for a program that the chip `Workers` of the person does not hold
//! starts nothing: it draws the name of the program, the first line of the task
//! and the three controls that decide it, and it draws one line of its state after
//! the press.
//!
//! Every text here comes from a program or from the settings of the person, so the
//! conversation cut it before the list held it, and this module draws it as plain
//! text on the rows of [`super::list`] and [`super::proposal`]: it copies neither
//! the bubble of the person, nor the text of the agent, nor the box and the
//! controls of a proposal.

use egui::{Rect, Sense, Ui, pos2, vec2};
use horizon_core::agent_bridge::WorkerAskState;

use crate::theme;
use crate::typeface::Weight;

use super::list::{self, CARD_INDENT};
use super::proposal;
use super::text;

/// The caption above a request for a worker.
pub(super) const CAPTION: &str = "The orchestrator wants to start a worker";
/// The controls of a request for a worker.
pub(super) const START: &str = "Start";
pub(super) const ALWAYS: &str = "Always for this work item";
pub(super) const NOT_NOW: &str = "Not now";
/// The state line of a request after the press.
pub(super) const STARTED: &str = "Started";
pub(super) const NOT_STARTED: &str = "Not started";
/// The words of the line of a request that the person allowed for this work item.
pub(super) const ALWAYS_LINE: &str = "is allowed for this work item";
/// The right end of the reference row of a worker whose panel stands on the canvas.
pub(super) const WORKER: &str = "worker";

const CAPTION_SIZE: f32 = 11.0;
const PROGRAM_SIZE: f32 = 12.5;
const TASK_SIZE: f32 = 11.5;
/// The air between the caption and the name of the program, and between it and the
/// task.
const ITEM_GAP: f32 = 8.0;
/// The air between the task and the controls.
const CONTROLS_GAP: f32 = 10.0;
/// The rows of the task that a request draws.
const TASK_ROWS: usize = 2;
const TASK_LINE_HEIGHT: f32 = 1.45;
const STATE_SIZE: f32 = 11.5;
/// The caption above a message to a worker, and above a report of a worker.
const TO_SIZE: f32 = 10.5;

/// What the person pressed on the row of a request for a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Choice {
    /// Start this one worker.
    Start,
    /// Add the program to the chip and start the worker.
    Always,
    /// Start nothing.
    NotNow,
}

/// The line under the controls of a request that the person answered, or `None`
/// while it waits for the press.
pub(super) fn state_line(state: WorkerAskState, program: &str) -> Option<String> {
    match state {
        WorkerAskState::Open => None,
        WorkerAskState::Started => Some(STARTED.to_owned()),
        WorkerAskState::Always => Some(format!("{STARTED}; {program} {ALWAYS_LINE}")),
        WorkerAskState::Declined => Some(NOT_STARTED.to_owned()),
    }
}

/// The galleys of one request: its caption, the name of the program, the first
/// rows of the task and the three controls.
struct Card {
    caption: std::sync::Arc<egui::Galley>,
    program: std::sync::Arc<egui::Galley>,
    task: std::sync::Arc<egui::Galley>,
    controls: Vec<std::sync::Arc<egui::Galley>>,
}

impl Card {
    fn new(ui: &Ui, program: &str, task: &str, inner: f32) -> Self {
        let label = |text: &str| {
            text::line(
                ui,
                text,
                proposal::BUTTON_SIZE,
                Weight::Semibold,
                theme::FG(),
                inner - 2.0 * proposal::BUTTON_PADDING,
            )
        };
        Self {
            caption: text::line(ui, CAPTION, CAPTION_SIZE, Weight::Regular, theme::FG_DIM(), inner),
            program: text::line(ui, program, PROGRAM_SIZE, Weight::Semibold, theme::FG(), inner),
            task: text::rows(
                ui,
                task,
                text::Metrics {
                    size: TASK_SIZE,
                    weight: Weight::Regular,
                    color: theme::FG_SOFT(),
                    line_height: TASK_LINE_HEIGHT,
                },
                inner,
                TASK_ROWS,
            ),
            controls: [START, ALWAYS, NOT_NOW].iter().map(|text| label(text)).collect(),
        }
    }

    /// The height of the text of the request, from its caption to its task.
    fn text_height(&self) -> f32 {
        self.caption.size().y + ITEM_GAP + self.program.size().y + self.task.size().y
    }
}

/// Draws the row of one request for a worker in `width` points, and answers the
/// control that the person pressed. A request that the person decided, and one
/// that a restart loaded as open, draws its state line and takes no press.
pub(super) fn ask_row(
    ui: &mut Ui,
    id: &str,
    program: &str,
    task: &str,
    state: WorkerAskState,
    width: f32,
    loaded_open: bool,
) -> Option<Choice> {
    let row_width = (width - CARD_INDENT).max(1.0);
    let inner = (row_width - 2.0 * proposal::PADDING).max(1.0);
    let card = Card::new(ui, program, task, inner);
    // A request that a restart loaded as open starts nothing: the program that
    // asked is gone, so the row reads `Not started` and takes no press.
    let line = match (state, loaded_open) {
        (WorkerAskState::Open, true) => Some(NOT_STARTED.to_owned()),
        (state, _) => state_line(state, program),
    };
    let state_line = line.map(|line| text::line(ui, &line, STATE_SIZE, Weight::Regular, theme::FG_DIM(), inner));
    let (controls_height, _) = proposal::control_boxes(0.0, 0.0, inner, &card.controls);
    let height = 2.0 * proposal::PADDING
        + card.text_height()
        + CONTROLS_GAP
        + match &state_line {
            Some(line) => line.size().y,
            None => controls_height,
        };
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, height));
    proposal::draw_box(ui.painter(), rect);
    let left = rect.min.x + proposal::PADDING;
    let mut at = rect.min.y + proposal::PADDING;
    let painter = ui.painter();
    painter.galley(pos2(left, at), card.caption.clone(), theme::FG_DIM());
    at += card.caption.size().y + ITEM_GAP;
    painter.galley(pos2(left, at), card.program.clone(), theme::FG());
    at += card.program.size().y;
    painter.galley(pos2(left, at), card.task.clone(), theme::FG_SOFT());
    if let Some(line) = state_line {
        let top = rect.max.y - proposal::PADDING - line.size().y;
        painter.galley(pos2(left, top), line, theme::FG_DIM());
        return None;
    }
    let top = rect.max.y - proposal::PADDING - controls_height;
    let (_, boxes) = proposal::control_boxes(left, top, inner, &card.controls);
    proposal::paint_controls(ui, &boxes, &card.controls, ("orchestrator_dock_worker_ask", id), true).map(|index| {
        match index {
            0 => Choice::Start,
            1 => Choice::Always,
            _ => Choice::NotNow,
        }
    })
}

/// Draws the reference row of one worker: the glyph of a panel, the name of its
/// preset and the word `worker` at its right end. A worker whose panel left the
/// board reads dim and takes no press, as the row of a card that is gone does. It
/// answers whether the person pressed the row.
pub(super) fn started_row(ui: &mut Ui, panel: &str, program: &str, width: f32, present: bool) -> bool {
    let state = if present { WORKER } else { list::CARD_STATE_CLOSED };
    list::reference_row(ui, ("orchestrator_dock_worker", panel), program, state, width, present)
}

/// Draws one message that the orchestrator typed into a worker: a bubble of the
/// person side under the caption `to <program>`.
pub(super) fn message_row(ui: &mut Ui, program: &str, text: &str, width: f32) {
    let caption = format!("to {program}");
    list::person_row(ui, text, Some(&caption), TO_SIZE, width);
}

/// Draws one report of a worker: a text of the agent under the caption
/// `<program> reports`.
pub(super) fn report_row(ui: &mut Ui, program: &str, text: &str, width: f32) {
    let caption = format!("{program} reports");
    list::agent_row(ui, text, Some(&caption), TO_SIZE, width);
}

#[cfg(test)]
mod tests {
    use super::{ALWAYS_LINE, NOT_STARTED, STARTED, state_line};
    use horizon_core::agent_bridge::WorkerAskState;

    #[test]
    fn the_state_of_a_request_reads_what_the_person_pressed() {
        assert_eq!(state_line(WorkerAskState::Open, "Codex"), None);
        assert_eq!(state_line(WorkerAskState::Started, "Codex"), Some(STARTED.to_owned()));
        assert_eq!(
            state_line(WorkerAskState::Always, "Codex"),
            Some(format!("Started; Codex {ALWAYS_LINE}"))
        );
        assert_eq!(
            state_line(WorkerAskState::Declined, "Codex"),
            Some(NOT_STARTED.to_owned())
        );
    }
}
