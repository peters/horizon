//! The three operations of the front desk: the work items that it reads, the
//! proposal that it offers to the person, and the message that it passes to one
//! work item.
//!
//! A work item is a workspace with its orchestrator conversation. The front desk
//! reads the state that the frame already holds and creates nothing: a proposal
//! becomes one entry of its conversation, and Horizon makes a workspace for an
//! item only when the person presses the control of that row. The message of the
//! front desk goes to the orchestrator of one work item through the one send of
//! the dock, with the caption `via the front desk` on the bubble.
//!
//! Every text here comes from a program: the title, the task and the folder of an
//! item, and the message that the front desk passes. The bounds of the request are
//! checked before any of it reaches the conversation or the board.

use std::time::Instant;

use horizon_browser_control::manifest::block::{
    MAX_WORK_ITEM_QUESTION_CHARS, MAX_WORK_ITEMS, Operation, Outcome, ProposedItem, WORK_ITEM_MESSAGE_INTERVAL_MILLIS,
    WorkItemSummary,
};
use horizon_browser_control::manifest::block_proposal;
use horizon_core::agent_bridge::ProposalItem;

use super::super::HorizonApp;
use super::caller_unavailable;

/// The words of Horizon that a proposal answers: the person decides, and one
/// message follows when they do.
pub(in crate::app) const PROPOSAL_ANSWER: &str = "The person decides. You get one message when they do.";

/// The messages that the front desk passed to each work item, and when.
///
/// The front desk may pass one message to one work item every ten seconds, so a
/// program that repeats itself fills no conversation. The clock is a value of
/// each call, so a test moves it forward instead of waiting.
#[derive(Default)]
pub(in crate::app) struct WorkItemMessages {
    sent: std::collections::HashMap<String, Instant>,
}

impl WorkItemMessages {
    /// Whether the front desk may pass a message to `work_item` at `now`: the
    /// first message of a work item passes, and so does one that follows the
    /// interval of the last.
    pub(in crate::app) fn due(&self, work_item: &str, now: Instant) -> bool {
        match self.sent.get(work_item) {
            Some(at) => now.saturating_duration_since(*at).as_millis() >= u128::from(WORK_ITEM_MESSAGE_INTERVAL_MILLIS),
            None => true,
        }
    }

    /// Records one message that went to `work_item` at `now`.
    pub(in crate::app) fn record(&mut self, work_item: &str, now: Instant) {
        self.sent.insert(work_item.to_owned(), now);
    }

    /// Forgets the work items that the board no longer holds, so the map stays
    /// as small as the board.
    pub(in crate::app) fn retain(&mut self, held: impl Fn(&str) -> bool) {
        self.sent.retain(|work_item, _| held(work_item));
    }
}

/// `text` cut to `max` characters, without a control character. A question of a card
/// and the last text of an orchestrator are text of a program, so each is cut before
/// an answer of this tool holds it.
fn cut(text: &str, max: usize) -> String {
    text.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(max)
        .collect()
}

impl HorizonApp {
    /// Answers one request of the front desk: its three operations, and the
    /// refusal of every other operation of the tool.
    pub(super) fn answer_front_desk(&mut self, operation: &Operation, ctx: &egui::Context) -> Outcome {
        match operation {
            Operation::WorkItems => self.answer_work_items(),
            Operation::WorkItemPropose { items } => self.answer_work_item_propose(items),
            Operation::WorkItemMessage { work_item, text } => self.answer_work_item_message(work_item, text, ctx),
            _ => caller_unavailable(),
        }
    }

    /// Answers `work_items`: the work items of this board, in the order of the
    /// sidebar, with the state that the word of their row shows. It reads the
    /// board, the questions of the cards and the conversations of the dock, so it
    /// reads no file and no process.
    fn answer_work_items(&self) -> Outcome {
        let questions = self.read_questions();
        let items = self
            .board
            .workspaces
            .iter()
            .take(MAX_WORK_ITEMS)
            .map(|workspace| {
                let asked: Vec<&super::super::sidebar::inbox::Question> = questions
                    .iter()
                    .filter(|question| question.workspace == workspace.id)
                    .collect();
                WorkItemSummary {
                    work_item: workspace.local_id.clone(),
                    name: workspace.name.clone(),
                    state: self
                        .workspace_orchestrator_with(workspace.id, asked.len())
                        .state()
                        .to_owned(),
                    question: asked
                        .first()
                        .map(|question| cut(&question.text, MAX_WORK_ITEM_QUESTION_CHARS)),
                    last: self.orchestrator_dock.last_text_of(&workspace.local_id),
                    cost_usd: self.orchestrator_dock.cost_of(&workspace.local_id),
                    machine: workspace.place.machine().map(str::to_owned),
                }
            })
            .collect();
        Outcome::WorkItems { items }
    }

    /// Answers `work_item_propose`: the items become one proposal in the
    /// conversation of the front desk, and nothing else changes. An older
    /// proposal that is still open becomes replaced.
    fn answer_work_item_propose(&mut self, items: &[ProposedItem]) -> Outcome {
        if let Err(refusal) = block_proposal::check_items(items) {
            return Outcome::failed(refusal.code, &refusal.message);
        }
        let items: Vec<ProposalItem> = items
            .iter()
            .map(|item| ProposalItem {
                title: item.title.clone(),
                task: item.task.clone(),
                folder: item.folder.clone(),
            })
            .collect();
        let proposal = horizon_core::new_local_id();
        self.orchestrator_dock.propose(&proposal, &items);
        Outcome::Proposed {
            proposal,
            text: PROPOSAL_ANSWER.to_owned(),
        }
    }

    /// Answers `work_item_message`: the text goes to the orchestrator of one work
    /// item as a message of the person, marked as a message that the front desk
    /// passed. A work item that the board does not hold, a conversation that runs
    /// a turn and a second message within the interval send nothing.
    fn answer_work_item_message(&mut self, work_item: &str, text: &str, ctx: &egui::Context) -> Outcome {
        if let Err(refusal) = block_proposal::check_message(text) {
            return Outcome::failed(refusal.code, &refusal.message);
        }
        let Some(workspace) = self.board.workspace_id_by_local_id(work_item) else {
            return Outcome::failed("not_found", "No workspace of this Horizon holds that work item");
        };
        if self.orchestrator_dock.busy(work_item) {
            return Outcome::failed(
                "busy",
                "The orchestrator of that work item runs a turn; its answer comes first",
            );
        }
        let now = Instant::now();
        if !self.front_desk_messages.due(work_item, now) {
            return Outcome::failed(
                "too_soon",
                "One message to a work item every ten seconds; that work item read a message of yours a moment ago",
            );
        }
        let local_id = self
            .board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone())
            .unwrap_or_default();
        if !self.say_to_orchestrator_marked(ctx, &local_id, text, true) {
            return Outcome::failed("busy", "The orchestrator of that work item took no message");
        }
        self.front_desk_messages.record(work_item, now);
        Outcome::WorkItemMessage {
            work_item: work_item.to_owned(),
        }
    }
}
