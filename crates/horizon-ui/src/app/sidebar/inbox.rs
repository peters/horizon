//! The group `Needs you` at the top of the sidebar: the questions that the cards
//! of this board ask the person, with the buttons that answer them.
//!
//! A question is a card that is open, whose document is not `finished`, and that
//! holds a callout with at least one answer that the person did not press. The
//! first such callout of the card is its question, and whether the person
//! answered it comes from the one record that `CardRuntime` keeps, as the row of
//! the card reads it.
//!
//! The group holds the three oldest questions, so the person answers a question
//! from wherever the person looks. A button of an item is the button of the card:
//! the press records the same click, on the one path of
//! [`crate::app::block_clicks`], and a press beside the buttons brings the card
//! into view. Every text of a callout is untrusted text of a program: it enters
//! this module as plain text without a control character, cut before layout.
//!
//! The questions are read with the rows of the workspace list ([`super::list`]),
//! at most once a second. A frame lays out the cached texts of a question, so a
//! frame that changes nothing builds no string.

use horizon_browser_control::block::Component;
use horizon_core::{BlockPanelState, PanelId, WorkspaceId};

use crate::theme;

use super::super::HorizonApp;
use super::SidebarActions;
use super::{caption, text};

mod item;

/// The title of the group, in the form of a caption of the sidebar.
pub(in crate::app) const GROUP: &str = caption::NEEDS_YOU;
/// The control that opens the card of a question with more than two answers.
pub(in crate::app) const OPEN: &str = "Open";
/// How many questions the group draws before its line `and <N> more`.
pub(in crate::app) const MAX_ITEMS: usize = 3;
/// The answers that an item draws; a callout with more answers gets `Open`.
pub(in crate::app) const MAX_ANSWERS: usize = 2;
/// The most characters of the text of a callout that an item keeps.
pub(in crate::app) const MAX_TEXT_CHARS: usize = 160;
/// The most characters of a button label that an item keeps.
pub(in crate::app) const MAX_LABEL_CHARS: usize = 24;
/// The air between two items.
const ITEM_GAP: f32 = 6.0;
/// The size and the left inset of the line `and <N> more`.
const MORE_SIZE: f32 = 11.0;
const MORE_INSET: f32 = 16.0;
/// One answer of a callout, as an item draws it: the id that the card reads back
/// and the label that the person reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Answer {
    pub(in crate::app) id: String,
    pub(in crate::app) label: String,
}

/// One question that the group shows: the callout of a card, the answers of it,
/// and the one line that a screen reader says.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Question {
    /// The card that asks.
    pub(in crate::app) panel: PanelId,
    /// The workspace of the card, whose row says `Needs you` with its question.
    pub(in crate::app) workspace: WorkspaceId,
    /// The name of the workspace, at the first line of the item.
    pub(in crate::app) name: String,
    /// The text of the callout, without a control character and cut to
    /// [`MAX_TEXT_CHARS`].
    pub(in crate::app) text: String,
    /// The first two answers of the callout, in its order, with the label of each
    /// cut to [`MAX_LABEL_CHARS`].
    pub(in crate::app) answers: Vec<Answer>,
    /// Whether the callout holds more answers than the item draws, so the item
    /// adds the control [`OPEN`].
    pub(in crate::app) more: bool,
    /// `<name>: <text>`, which a screen reader says for the item.
    pub(in crate::app) spoken: String,
}

impl HorizonApp {
    /// Reads the questions of this board, the oldest first: one for each card that
    /// is open, whose document is not finished, and that holds a callout whose
    /// answer nobody pressed.
    ///
    /// The order is the time of the last update of a card, which is the time that
    /// the document with that callout arrived, and after it the order of the
    /// board.
    pub(in crate::app) fn read_questions(&self) -> Vec<Question> {
        let mut questions: Vec<(i64, usize, Question)> = Vec::new();
        for (index, panel) in self.board.panels.iter().enumerate() {
            let Some(card) = panel.block() else {
                continue;
            };
            // A card of Horizon's own asks for a setting and answers on its own
            // path, and a folded card shows no callout.
            if card.owner_actor.is_none() || card.document.finished || card.folded() {
                continue;
            }
            // An owner that is gone reads no answer: the program of the card left,
            // so its buttons are dead and the question is no question of the person.
            if !self.card_owner_present(panel) {
                continue;
            }
            let Some(workspace) = self.board.workspace(panel.workspace_id) else {
                continue;
            };
            let Some(callout) = self.first_unanswered(panel.id, card) else {
                continue;
            };
            let question = question_of(panel.id, panel.workspace_id, &workspace.name, callout);
            questions.push((card.updated_at_millis, index, question));
        }
        questions.sort_by_key(|(at, index, _)| (*at, *index));
        questions.into_iter().map(|(_, _, question)| question).collect()
    }

    /// The first callout of `card` whose answer nobody pressed.
    fn first_unanswered<'a>(&self, panel: PanelId, card: &'a BlockPanelState) -> Option<&'a Component> {
        card.document.components.iter().find(|component| {
            matches!(
                component,
                Component::Callout { buttons, .. } if !buttons.is_empty() && !answered(self, panel, component)
            )
        })
    }

    /// Draws the group above the heading of the workspaces, while one card of the
    /// board asks a question. It takes no height and builds no text otherwise, and
    /// it answers whether it drew the group.
    pub(super) fn render_sidebar_inbox(&mut self, ui: &mut egui::Ui, actions: &mut SidebarActions) -> bool {
        let questions = std::mem::take(&mut self.sidebar_list.questions);
        if questions.is_empty() {
            return false;
        }
        let mut asked: Option<(PanelId, String)> = None;
        let mut opened: Option<PanelId> = None;

        let count = self.sidebar_chrome.counts.text(caption::NEEDS_YOU, questions.len());
        caption::group(ui, GROUP, Some(count), &[]);
        for (index, question) in questions.iter().take(MAX_ITEMS).enumerate() {
            if index > 0 {
                ui.add_space(ITEM_GAP);
            }
            let press = item::render(ui, question);
            if let Some(id) = press.answer {
                asked = Some((question.panel, id));
            }
            if press.open {
                opened = Some(question.panel);
            }
        }
        if questions.len() > MAX_ITEMS {
            ui.add_space(ITEM_GAP);
            render_more(ui, questions.len() - MAX_ITEMS);
        }

        self.sidebar_list.questions = questions;
        if let Some((panel, id)) = asked {
            // The press of an answer of the card, on the one path of the card: the
            // runtime of the card holds the click, the answer and the note of it,
            // and the frames settle them.
            self.press_card_button(panel, &id);
            ui.ctx().request_repaint();
        }
        if let Some(panel) = opened {
            // The card comes into view on the path of a workspace row: its
            // workspace becomes the active one and the canvas shows the panel.
            actions.focus_panel = Some(panel);
            actions.pan_to_panel = Some(panel);
        }
        true
    }
}

/// Whether the person pressed an answer of `callout`, as the live card of this
/// run or the saved card holds it. It is the rule of the attention of a card, so
/// a question leaves the group at the same press that ends the mark of its row.
fn answered(app: &HorizonApp, panel: PanelId, callout: &Component) -> bool {
    let live = app
        .panel_render_caches
        .card_runtime
        .get(&panel)
        .is_some_and(|runtime| runtime.pressed_answer_of(callout));
    let saved = app
        .board
        .panel(panel)
        .and_then(|panel| panel.block())
        .is_some_and(|card| card.pressed_answer_of(callout));
    live || saved
}

/// The question of one card, with the texts that an item draws.
fn question_of(panel: PanelId, workspace: WorkspaceId, name: &str, callout: &Component) -> Question {
    let (text, buttons) = match callout {
        Component::Callout { text, buttons, .. } => (text.as_str(), buttons.as_slice()),
        _ => ("", &[][..]),
    };
    let text = plain(text, MAX_TEXT_CHARS);
    let answers: Vec<Answer> = buttons
        .iter()
        .take(MAX_ANSWERS)
        .map(|button| Answer {
            id: button.id.clone(),
            label: plain(&button.label, MAX_LABEL_CHARS),
        })
        .collect();
    Question {
        panel,
        workspace,
        name: name.to_owned(),
        spoken: format!("{name}: {text}"),
        text,
        more: buttons.len() > MAX_ANSWERS,
        answers,
    }
}

/// `text` with each control character replaced by a space, cut to `max`
/// characters. The text of a callout is text of a program, so it is cut before
/// any layout reads its length.
fn plain(text: &str, max: usize) -> String {
    text.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(max)
        .collect()
}

/// Draws the line that names the questions under the third item.
fn render_more(ui: &mut egui::Ui, rest: usize) {
    ui.horizontal(|ui| {
        ui.add_space(MORE_INSET);
        ui.label(text::line(
            ui,
            &format!("and {rest} more"),
            MORE_SIZE,
            text::REGULAR,
            theme::FG_DIM(),
            160.0,
        ));
    });
}

/// A card of `workspace` that asks `text` with the answers `answers`, for the
/// tests of the sidebar: an open card whose orchestrator owns it.
#[cfg(test)]
pub(in crate::app) fn asking_card(
    app: &mut HorizonApp,
    workspace: WorkspaceId,
    text: &str,
    answers: &[(&str, &str)],
) -> PanelId {
    use horizon_browser_control::block::{BlockDocument, Button, Tone};

    let local_id = app.board.workspace(workspace).expect("workspace").local_id.clone();
    // A question is the question of a card whose owner is here, so the test holds
    // the program of its orchestrator without starting one.
    app.orchestrator_dock.mark_running_for_test(&local_id);
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Callout {
            tone: Tone::Neutral,
            caption: None,
            text: text.to_owned(),
            buttons: answers
                .iter()
                .map(|(id, label)| Button {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                    tone: Tone::Neutral,
                })
                .collect(),
        }],
    };
    let state = BlockPanelState {
        owner_actor: horizon_core::agent_bridge::actor::of_workspace(&local_id),
        ..BlockPanelState::new(document)
    };
    app.board
        .create_panel(
            horizon_core::PanelOptions {
                kind: horizon_core::PanelKind::Block,
                block: Some(state),
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .expect("card")
}

#[cfg(test)]
mod tests;
