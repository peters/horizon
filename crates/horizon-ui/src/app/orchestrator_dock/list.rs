//! The list of the dock: the entries of one conversation, from the top of the
//! body to its bottom edge.
//!
//! The list scrolls between the header and the composer and it follows the
//! bottom while the person has not scrolled up (`ScrollArea::stick_to_bottom`).
//! One entry takes one row: a message of the person at the right, a text of the
//! agent at the left, one tool call, or a notice of Horizon centred over two
//! lines. A reference row of a card and a reference row of a work item take a row
//! that the person presses to bring that panel or that workspace into view, and
//! the proposal of the front desk takes the row of the two controls that start it
//! or leave it.
//!
//! Every text of the program is untrusted, so this module draws it as plain text
//! only: no markdown, no link and no path becomes active. One frame lays out at
//! most [`MAX_DRAWN_ENTRIES`] entries, which are the last ones of the list, and
//! one notice above them names the entries that it does not draw. The list
//! itself keeps [`horizon_core::agent_bridge::MAX_ENTRIES`] entries.

mod notice_rows;
mod reference_rows;
mod text_rows;
mod tool_rows;

use egui::{Color32, Rect, Ui, UiBuilder, pos2};
use horizon_core::agent_bridge::{Entry, ProposalState, WorkerAskState};

use crate::loading_spinner;
use crate::theme;
use horizon_core::Board;

use self::notice_rows::{notice_row, working_row};
pub(super) use self::reference_rows::reference_row;
use self::reference_rows::{missing_work_item_row, work_item_row};
pub(super) use self::text_rows::{agent_row, person_row};
use self::tool_rows::tool_row;
use super::OrchestratorDock;
use super::proposal;
use super::worker_rows;

/// What a press on one row of the list asks the frame to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Press {
    /// Bring the panel of this reference row into view: a card of the program, or
    /// the panel of one of its workers.
    Card(String),
    /// Show the workspace of this work item, and make it the active one.
    WorkItem(String),
    /// Start the work items of this proposal, or leave them.
    Proposal { id: String, choice: proposal::Choice },
    /// Start one worker that a request asked for, or leave it.
    WorkerAsk { id: String, choice: worker_rows::Choice },
}

/// The facts that the reference rows of the list read of the board and of the
/// dock, gathered once for the frame that draws them.
#[derive(Default)]
pub(super) struct Refs {
    /// The reference row of each work item of the conversation.
    work_items: Vec<WorkItemRow>,
    /// The proposals and the requests for a worker that the conversation loaded
    /// from its file: the program that asked is gone, so they take no press.
    loaded: Vec<String>,
}

/// One reference row of a work item: its name, the state word of its workspace
/// while the board holds it, and the mark of that workspace.
#[derive(Clone, Debug)]
pub(super) struct WorkItemRow {
    work_item: String,
    name: String,
    /// Whether the board holds the workspace of this work item.
    held: bool,
    /// The state word of the work item, or `None` when the board holds no
    /// workspace of it, or when that workspace says no word.
    word: Option<&'static str>,
    accent: Color32,
}

impl Refs {
    /// The facts of the reference rows of the conversation that the dock holds of
    /// `local_id` on `board`.
    pub(super) fn build(board: &Board, dock: &OrchestratorDock, local_id: &str) -> Self {
        let mut refs = Self::default();
        let entries = dock.entries_of(local_id);
        let loaded = dock.loaded_entries(local_id);
        for (index, entry) in entries.entries.iter().enumerate() {
            match entry {
                Entry::WorkItem { work_item, name } => {
                    let workspace = board.workspace_id_by_local_id(work_item.as_str());
                    let accent = workspace
                        .and_then(|workspace| board.workspace(workspace))
                        .map_or_else(theme::FG_DIM, |workspace| theme::workspace_accent(workspace.color_idx));
                    refs.work_items.push(WorkItemRow {
                        work_item: work_item.clone(),
                        name: name.clone(),
                        held: workspace.is_some(),
                        word: workspace.and_then(|_| dock.work_item_word(work_item.as_str())),
                        accent,
                    });
                }
                Entry::Proposal { id, state, .. } if *state == ProposalState::Open && index < loaded => {
                    refs.loaded.push(id.clone());
                }
                Entry::WorkerAsk {
                    id,
                    state: WorkerAskState::Open,
                    ..
                } if index < loaded => {
                    refs.loaded.push(id.clone());
                }
                _ => {}
            }
        }
        refs
    }

    /// The reference row of `work_item`, when the conversation holds one.
    fn work_item(&self, work_item: &str) -> Option<&WorkItemRow> {
        self.work_items.iter().find(|row| row.work_item == work_item)
    }

    /// Whether the conversation loaded the proposal `id`, or the request `id` of a
    /// worker, from its file.
    fn loaded_open(&self, id: &str) -> bool {
        self.loaded.iter().any(|held| held == id)
    }
}

/// The padding of the list at each side, above the first entry and under the
/// last one.
pub(super) const LIST_PADDING: f32 = 16.0;
/// The air between two entries.
pub(super) const ENTRY_GAP: f32 = 16.0;
/// The air between two reference rows that follow each other: the rows of one
/// group belong together.
pub(super) const REF_GAP: f32 = 6.0;
/// The air between two tool rows that follow each other.
const TOOL_GAP: f32 = 4.0;
/// The entries that one frame lays out.
const MAX_DRAWN_ENTRIES: usize = 200;

/// The share of the list width that a bubble of the person takes at most.
const PERSON_SHARE: f32 = 0.86;
const BUBBLE_RADIUS: u8 = 12;
pub(super) const BUBBLE_PADDING_X: f32 = 12.0;
const BUBBLE_PADDING_Y: f32 = 9.0;

/// The share of the list width that a text of the agent takes at most.
const AGENT_SHARE: f32 = 0.96;
const AGENT_BAR_WIDTH: f32 = 3.0;
const AGENT_BAR_RADIUS: u8 = 2;
const AGENT_GAP: f32 = 10.0;

const BODY_SIZE: f32 = 13.5;
const PERSON_LINE_HEIGHT: f32 = 1.5;
const AGENT_LINE_HEIGHT: f32 = 1.55;

const TOOL_INDENT: f32 = 13.0;
const TOOL_DOT: f32 = 6.0;
const TOOL_FONT_SIZE: f32 = 12.0;
/// The font of the line of a result under a tool row that failed.
const TOOL_RESULT_FONT_SIZE: f32 = 11.5;
/// The air between a tool row and the line of its result.
const TOOL_RESULT_GAP: f32 = 2.0;
const TOOL_GAP_AFTER_DOT: f32 = 8.0;
const TOOL_GAP_AFTER_NAME: f32 = 8.0;
/// The mauve of the workspace accents, which the bar of an agent text ends at.
const AGENT_BAR_END_ACCENT: usize = 6;

/// The air between a reference row of a card and the left edge of the list: the
/// indent of a tool row.
pub(super) const CARD_INDENT: f32 = TOOL_INDENT;
/// The share of the list width that a reference row of a card takes at most.
const CARD_SHARE: f32 = 0.96;
/// The height of one reference row.
pub(super) const CARD_HEIGHT: f32 = 34.0;
pub(super) const CARD_RADIUS: u8 = 10;
/// The alpha of the fill of a row whose card stands on the canvas.
const CARD_FILL_ALPHA: u8 = 18;
/// The alpha of its outline.
const CARD_OUTLINE_ALPHA: u8 = 115;
/// The padding of a row at its left and at its right.
pub(super) const CARD_PADDING_X: f32 = 10.0;
/// The size of the glyph of a card: a small card at the left of the row.
const CARD_GLYPH: [f32; 2] = [22.0, 16.0];
const CARD_GLYPH_RADIUS: u8 = 4;
const CARD_GLYPH_STROKE: f32 = 1.5;
/// How far under the top edge of the glyph its one line stands.
const CARD_GLYPH_LINE_DROP: f32 = 3.0;
/// The air between that line and the two sides of the glyph.
const CARD_GLYPH_LINE_INSET: f32 = 4.0;
/// The air between the glyph and the heading of the card.
pub(super) const CARD_GAP_AFTER_GLYPH: f32 = 10.0;
const CARD_TITLE_SIZE: f32 = 12.5;
const CARD_STATE_SIZE: f32 = 11.0;
/// The right end of a row whose card stands on the canvas.
pub(super) const CARD_STATE: &str = "on the canvas";
/// The right end of a row whose panel left the board.
pub(super) const CARD_STATE_CLOSED: &str = "closed";

/// The side of the mark of the workspace of a work item.
const WORK_ITEM_TILE: f32 = 20.0;
/// The size of the state word at the right end of a row of a work item.
const WORK_ITEM_STATE_SIZE: f32 = 11.0;
/// The caption above a message of the person that the front desk passed.
const VIA_FRONT_DESK: &str = "via the front desk";
const VIA_SIZE: f32 = 10.5;
/// The air between the caption of a passed message and its bubble.
const VIA_GAP: f32 = 2.0;

const NOTICE_FONT_SIZE: f32 = 10.5;
const NOTICE_GAP: f32 = 8.0;
/// The least length of a line of a notice at one side of its text.
const NOTICE_MIN_LINE: f32 = 8.0;

/// The height of the row of the spinner, which is the diameter of its arc.
const WORKING_HEIGHT: f32 = loading_spinner::ARC_SPAN;
const WORKING_GAP: f32 = 8.0;
const WORKING_SIZE: f32 = 12.0;

/// Draws the entries of `entries` in `body`, between the header and the
/// composer, with the row of `working` under them while a turn waits for the
/// first text of the agent. `local_id` names the workspace of the conversation:
/// the place of the list belongs to that workspace, so the scroll of one
/// conversation never shows in another. `refs` holds the state of each work item
/// and the proposals that came from a file, and `card_present` says whether the
/// board still holds a card of a reference row.
///
/// It answers the row that the person pressed, so the frame acts on it.
pub(super) fn render(
    ui: &mut Ui,
    body: Rect,
    local_id: &str,
    entries: &[Entry],
    working: bool,
    refs: &Refs,
    card_present: &dyn Fn(&str) -> bool,
) -> Option<Press> {
    let width = (body.width() - 2.0 * LIST_PADDING).max(0.0);
    if width <= 0.0 || body.height() <= 0.0 {
        return None;
    }
    let area = Rect::from_min_max(
        pos2(body.min.x + LIST_PADDING, body.min.y),
        pos2(body.max.x - LIST_PADDING, body.max.y),
    );
    let mut area_ui = ui.new_child(UiBuilder::new().max_rect(area));
    area_ui.shrink_clip_rect(area);
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt(("orchestrator_dock_list", local_id))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(&mut area_ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            let skipped = entries.len().saturating_sub(MAX_DRAWN_ENTRIES);
            ui.add_space(LIST_PADDING);
            if skipped > 0 {
                notice_row(ui, &skipped_notice(skipped), width);
            }
            let mut previous: Option<&Entry> = None;
            for entry in &entries[skipped..] {
                ui.add_space(gap(previous, entry));
                if let Some(press) = entry_row(ui, entry, width, refs, card_present) {
                    picked = Some(press);
                }
                previous = Some(entry);
            }
            if working {
                ui.add_space(if previous.is_some() { ENTRY_GAP } else { 0.0 });
                working_row(ui, width);
            }
            ui.add_space(LIST_PADDING);
        });
    picked
}

/// Whether a turn of this conversation waits for the first text of the agent:
/// it is busy, and no text of the agent came after the last message of the
/// person.
pub(super) fn waits_for_text(entries: &[Entry], busy: bool) -> bool {
    busy && !entries
        .iter()
        .rev()
        .take_while(|entry| !matches!(entry, Entry::Person { .. }))
        .any(|entry| matches!(entry, Entry::Agent { .. }))
}

/// The air above `entry`, which is wider between two entries than between two
/// tool rows or two reference rows that follow each other.
fn gap(previous: Option<&Entry>, entry: &Entry) -> f32 {
    match previous {
        None => 0.0,
        Some(Entry::Tool { .. }) if matches!(entry, Entry::Tool { .. }) => TOOL_GAP,
        Some(previous) if is_reference(previous) && is_reference(entry) => REF_GAP,
        Some(_) => ENTRY_GAP,
    }
}

/// Whether `entry` takes the row of a reference to a card or to a work item.
fn is_reference(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Card { .. } | Entry::WorkItem { .. } | Entry::WorkerStarted { .. }
    )
}

/// The text of the notice of the entries that one frame does not draw.
fn skipped_notice(skipped: usize) -> String {
    format!("{skipped} earlier entries are not drawn")
}

/// Draws one entry of the list in one row of `width` points, and answers the row
/// that the person pressed.
fn entry_row(
    ui: &mut Ui,
    entry: &Entry,
    width: f32,
    refs: &Refs,
    card_present: &dyn Fn(&str) -> bool,
) -> Option<Press> {
    match entry {
        Entry::Person { text, via_front_desk } => {
            let caption = via_front_desk.then_some(VIA_FRONT_DESK);
            person_row(ui, text, caption, VIA_SIZE, width);
            None
        }
        Entry::Agent { text, .. } => {
            agent_row(ui, text, None, VIA_SIZE, width);
            None
        }
        Entry::Tool {
            name,
            summary,
            result,
            state,
            ..
        } => {
            tool_row(ui, name, summary, result.as_deref(), *state, width);
            None
        }
        Entry::Notice { text } => {
            notice_row(ui, text, width);
            None
        }
        Entry::Card { panel, title } => {
            let state = if card_present(panel) {
                CARD_STATE
            } else {
                CARD_STATE_CLOSED
            };
            reference_row(
                ui,
                ("orchestrator_dock_card", panel),
                title,
                state,
                width,
                card_present(panel),
            )
            .then(|| Press::Card(panel.clone()))
        }
        Entry::WorkItem { work_item, .. } => {
            if let Some(row) = refs.work_item(work_item) {
                work_item_row(ui, row, width).then(|| Press::WorkItem(work_item.clone()))
            } else {
                // A row of a work item that this frame gathered no fact of still
                // takes its height, so the list does not move under the person.
                missing_work_item_row(ui, width);
                None
            }
        }
        Entry::Proposal { id, items, state } => proposal::render(ui, id, items, *state, width, refs.loaded_open(id))
            .map(|choice| Press::Proposal { id: id.clone(), choice }),
        Entry::WorkerStarted { panel, program, .. } => {
            worker_rows::started_row(ui, panel, program, width, card_present(panel)).then(|| Press::Card(panel.clone()))
        }
        Entry::WorkerMessage { program, text, .. } => {
            worker_rows::message_row(ui, program, text, width);
            None
        }
        Entry::WorkerReport { program, text, .. } => {
            worker_rows::report_row(ui, program, text, width);
            None
        }
        Entry::WorkerAsk {
            id,
            program,
            task,
            state,
        } => worker_rows::ask_row(ui, id, program, task, *state, width, refs.loaded_open(id))
            .map(|choice| Press::WorkerAsk { id: id.clone(), choice }),
    }
}

/// The name that the row of a tool shows. A tool of a server carries the name of
/// its server in front (`mcp__<server>__<tool>`), which fills the row and hides
/// the tool, so the row shows the part after the last `__`.
pub(super) fn tool_label(name: &str) -> &str {
    if !name.starts_with("mcp__") {
        return name;
    }
    match name.rsplit_once("__") {
        Some((_, tool)) if !tool.is_empty() => tool,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::agent_bridge::Entry;

    use super::{MAX_DRAWN_ENTRIES, skipped_notice, tool_label, waits_for_text};

    #[test]
    fn the_row_of_a_tool_of_a_server_shows_the_tool_and_not_the_server() {
        assert_eq!(
            tool_label("mcp__plugin_horizon_horizon-browser__block_panel"),
            "block_panel"
        );
        assert_eq!(tool_label("Bash"), "Bash");
        assert_eq!(tool_label("my__tool"), "my__tool", "only a tool of a server is cut");
        assert_eq!(
            tool_label("mcp__server__"),
            "mcp__server__",
            "a name with no tool stays whole"
        );
    }

    fn person(text: &str) -> Entry {
        Entry::Person {
            text: text.to_owned(),
            via_front_desk: false,
        }
    }

    fn agent(text: &str) -> Entry {
        Entry::Agent {
            message_id: "msg-1".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_row_of_work_shows_until_the_first_text_of_this_turn() {
        assert!(!waits_for_text(&[], false), "a quiet conversation shows no row of work");
        assert!(waits_for_text(&[person("alpha")], true));
        assert!(
            !waits_for_text(&[person("alpha"), agent("bravo")], true),
            "a text of this turn answers the row of work"
        );
        assert!(
            waits_for_text(&[agent("bravo"), person("alpha")], true),
            "the text of an earlier turn does not answer this one"
        );
        assert!(!waits_for_text(&[person("alpha"), agent("bravo")], false));
    }

    #[test]
    fn one_frame_draws_the_last_two_hundred_entries_and_names_the_others() {
        assert_eq!(MAX_DRAWN_ENTRIES, 200);
        assert_eq!(skipped_notice(3), "3 earlier entries are not drawn");
    }
}
