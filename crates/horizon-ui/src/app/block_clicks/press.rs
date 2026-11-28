//! The press path of a card: who owns the card, the chip of its titlebar, the click
//! that it holds, the answer that the person pressed and the line of the press in
//! the conversation of an orchestrator.
//!
//! The widget records a press in the runtime state of the card; this file reads that
//! state and settles it.

use horizon_browser_control::block::{BlockDocument, ClickEvent, Component};
use horizon_browser_control::manifest;
use horizon_core::agent_bridge::actor;
use horizon_core::node_cards;
use horizon_core::{BlockPanelState, CardRuntime, Panel, PanelId, PanelKind, browser_actor};

use crate::block_widget::{CardChip, chip::ChipState};

use super::super::HorizonApp;
use super::super::block_requests::card_owner;

/// The most characters of a button label and of a card heading that the line of
/// one press keeps.
const PRESS_LINE_CHARS: usize = 40;

impl HorizonApp {
    /// The agent panel that owns a card, while it is still in the card's
    /// workspace.
    pub(in crate::app) fn card_owner_panel(&self, panel: &Panel) -> Option<PanelId> {
        let owner = panel.block()?.owner_actor.as_deref()?;
        self.board
            .panels
            .iter()
            .find(|candidate| {
                candidate.workspace_id == panel.workspace_id
                    && candidate.kind.is_agent()
                    && browser_actor(&candidate.local_id) == owner
            })
            .map(|candidate| candidate.id)
    }

    /// Display name of the agent that owns a card, while its panel is in the
    /// card's workspace.
    pub(in crate::app) fn card_owner_name(&self, panel_id: PanelId) -> Option<&'static str> {
        let panel = self.board.panel(panel_id)?;
        let owner = self.card_owner_panel(panel)?;
        Some(self.board.panel(owner)?.kind.display_name())
    }

    /// The state chip a card shows in its titlebar: who owns it and whether
    /// that agent waits for a click right now.
    pub(in crate::app) fn card_titlebar_chip(&self, panel_id: PanelId) -> Option<CardChip<'_>> {
        let panel = self.board.panel(panel_id)?;
        let card = panel.block()?;
        // A folded card keeps the badge of its heading on its one line, in its tone.
        if let Some((_, badge)) = card.folded_line() {
            return badge.map(|badge| CardChip {
                name: &badge.text,
                state: ChipState::Badge(badge.tone),
            });
        }
        let owned_by_an_agent = card.owner_actor.is_some();
        // A card that Horizon wrote itself names Horizon, not an agent that is
        // not there: its buttons are Horizon's own controls.
        if !owned_by_an_agent {
            return Some(CardChip::of_horizon());
        }
        let listening = self.block_listening(panel_id);
        Some(match self.card_owner_panel(panel) {
            Some(owner) => CardChip::for_panel(self.board.panel(owner)?.kind, listening, true),
            // The owner is the orchestrator of this workspace: its program runs in
            // the dock with no terminal, so the chip names that program and says
            // whether it waits, and `No agent` once the program ended.
            None if card_owner(panel).and_then(actor::workspace_of).is_some() => {
                CardChip::for_panel(PanelKind::Claude, listening, self.card_owner_present(panel))
            }
            // The owner runs on a machine of the list, so the chip names that machine:
            // the session is no panel of this board.
            None if self.card_owner_present(panel) => {
                let (machine, _session) = card_owner(panel).and_then(node_cards::actor_parts)?;
                CardChip::on_machine(machine)
            }
            None => CardChip::for_panel(PanelKind::Block, false, false),
        })
    }

    /// Whether Horizon wrote the document of this card itself, so its buttons
    /// are Horizon's controls and no agent reads a press on them.
    pub(in crate::app) fn card_host_owned(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .and_then(|panel| panel.block())
            .is_some_and(|block| block.owner_actor.is_none())
    }

    /// Whether the owner of a card is here: a panel of its workspace, the bridge
    /// that the dock runs for its workspace, or the session of a machine that the
    /// watch reports. A session whose program is no agent reports no row, so the
    /// list of sessions is what settles it.
    pub(in crate::app) fn card_owner_present(&self, panel: &Panel) -> bool {
        if self.card_owner_panel(panel).is_some() {
            return true;
        }
        let Some(owner) = card_owner(panel) else {
            return false;
        };
        if let Some(local_id) = actor::workspace_of(owner) {
            // A program of the dock has no terminal and no panel of this board, so
            // the bridge of its conversation is its presence. That program reads and
            // closes only the cards of the workspace that its actor names, so a card
            // that the person moved to another workspace has no owner there, as a card
            // of a panel owner has none once that panel is in another workspace.
            return self.board.workspace_id_by_local_id(local_id) == Some(panel.workspace_id)
                && self.orchestrator_dock.running(local_id);
        }
        let Some((machine, session)) = node_cards::actor_parts(owner) else {
            return false;
        };
        self.machine_watch
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.machine(machine))
            .is_some_and(|held| held.sessions.iter().any(|name| name == session))
    }

    /// Whether the owner of a card is here, so its buttons are live.
    pub(in crate::app) fn card_agent_present(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .is_some_and(|panel| self.card_owner_present(panel))
    }

    /// Whether a `next_click` waits on a card, alone or on any card of its
    /// owner, so a click reaches its agent at once.
    pub(in crate::app) fn block_listening(&self, panel_id: PanelId) -> bool {
        if self.block_clicks.listening_on_card(panel_id) {
            return true;
        }
        self.board
            .panel(panel_id)
            .and_then(card_owner)
            .is_some_and(|owner| self.block_clicks.listening_on_owner(owner))
    }

    /// Clicks that wait for a card's agent, after dropping the ones that aged
    /// out.
    pub(in crate::app) fn card_pending_clicks(&mut self, panel_id: PanelId) -> usize {
        let now_millis = manifest::now_millis();
        self.panel_render_caches
            .card_runtime
            .get_mut(&panel_id)
            .map_or(0, |card| card.pending_clicks(now_millis))
    }

    /// The next click of a card as its agent reads it, removed from the card. A
    /// click reaches an agent at most once.
    pub(super) fn take_card_click(&mut self, panel_id: PanelId) -> Option<ClickEvent> {
        let panel = self.board.panel(panel_id)?;
        let revision = panel.block()?.revision;
        let local_id = panel.local_id.clone();
        let now_millis = manifest::now_millis();
        let click = self
            .panel_render_caches
            .card_runtime
            .get_mut(&panel_id)?
            .take_click(now_millis)?;
        Some(ClickEvent {
            event_id: click.event_id,
            panel_id: local_id,
            component_id: click.component_id,
            revision: click.revision,
            selected: click.selected,
            at_millis: click.at_millis,
            stale: click.revision < revision,
        })
    }

    /// Starts a new wake streak: a waiter existed, so the owner is listening
    /// again for the next click.
    pub(super) fn reset_card_wake_streak(&mut self, panel_id: PanelId) {
        if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&panel_id) {
            card.reset_wake_streak();
        }
    }

    /// Adds one line to the conversation of the workspace of a card of an
    /// orchestrator for each button that the person pressed there, so the person
    /// reads in the dock what the program was told. The line is Horizon's words:
    /// the label and the heading are text of the program, cut here.
    pub(super) fn note_card_presses(&mut self) -> bool {
        let presses: Vec<(PanelId, String)> = self
            .panel_render_caches
            .card_runtime
            .iter_mut()
            .filter_map(|(panel_id, card)| card.take_press_note().map(|label| (*panel_id, label)))
            .collect();
        let mut noted = false;
        for (panel_id, label) in presses {
            let Some(panel) = self.board.panel(panel_id) else {
                continue;
            };
            let Some(heading) = panel.block().map(|block| block.heading().to_owned()) else {
                continue;
            };
            let Some(local_id) = card_owner(panel).and_then(actor::workspace_of).map(str::to_owned) else {
                continue;
            };
            if self.board.workspace_id_by_local_id(&local_id) != Some(panel.workspace_id) {
                continue;
            }
            self.orchestrator_dock
                .session_notice(&local_id, &press_line(&label, &heading));
            noted = true;
        }
        noted
    }

    /// Carries the answer that the person pressed to the saved card, and the
    /// answer of a restored card back into the live one.
    ///
    /// A press belongs to the frame in which the person made it, so the widget
    /// records it in the runtime state of the card. A restart has no runtime
    /// state, so the answer is written to the saved card, which the sidebar reads
    /// after a restart, and a card that comes back with an answered callout
    /// starts quiet. Returns whether it saved an answer.
    pub(super) fn settle_card_answers(&mut self) -> bool {
        let cards = self.card_documents();
        if self.block_clicks.cards != cards {
            self.block_clicks.cards.clone_from(&cards);
            self.sidebar_list.count_inbox_change();
        }
        let mut saved = false;
        for (id, _) in cards {
            let live = self
                .panel_render_caches
                .card_runtime
                .get(&id)
                .and_then(CardRuntime::answered_callout);
            let recorded = self
                .board
                .panel(id)
                .and_then(Panel::block)
                .and_then(BlockPanelState::answered_callout);
            if live == recorded {
                continue;
            }
            if let Some(answer) = live.cloned() {
                if let Some(state) = self.board.panel_mut(id).and_then(Panel::block_mut) {
                    state.record_answer(answer);
                    saved = true;
                }
            } else if let Some(answer) = recorded.cloned()
                && let Some(card) = self.panel_render_caches.card_runtime.get_mut(&id)
            {
                card.record_answer(answer);
            }
        }
        if saved {
            self.mark_runtime_dirty();
        }
        saved
    }

    /// Records the press of the person on the button or the answer `component_id`
    /// of the card `panel_id`, exactly as a press on that button of the card
    /// records it: the runtime state of the card holds the click, the note of it
    /// and the answer that it is, and [`Self::settle_block_clicks`] settles them on
    /// this frame or a later one. It answers whether the card took the press.
    ///
    /// The group `Needs you` of the sidebar draws the buttons of a card that asks
    /// a question, so the person answers there on the one path of the card.
    pub(in crate::app) fn press_card_button(&mut self, panel_id: PanelId, component_id: &str) -> bool {
        let Some(panel) = self.board.panel(panel_id) else {
            return false;
        };
        let Some(card) = panel.block() else {
            return false;
        };
        let (revision, document) = (card.revision, &card.document);
        let label = button_label(document, component_id).map(str::to_owned);
        let runtime = self.panel_render_caches.card_runtime.entry(panel_id).or_default();
        if !runtime.press(component_id, revision, document, manifest::now_millis()) {
            return false;
        }
        if let Some(label) = label {
            runtime.note_press(&label);
        }
        // The answer of the person ends the question of the card and may end the
        // word of the row of its workspace, so the sidebar reads them again.
        self.sidebar_list.count_inbox_change();
        true
    }

    /// The cards of the board and the revision of each, in board order.
    fn card_documents(&self) -> Vec<(PanelId, u64)> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::Block)
            .map(|panel| (panel.id, panel.block().map_or(0, |card| card.revision)))
            .collect()
    }
    /// Drops the clicks that aged out, starts a new wake streak for a card that
    /// holds none, and forgets the cards that left the board.
    ///
    /// The runtime state of a card that is still on the board stays in the map:
    /// the widget inserts it again on the next frame, so dropping it there would
    /// churn the render path.
    pub(super) fn prune_card_clicks(&mut self, now_millis: i64) {
        let board = &self.board;
        self.panel_render_caches.card_runtime.retain(|panel_id, card| {
            if board.panel(*panel_id).is_none() {
                return false;
            }
            if card.pending_clicks(now_millis) == 0 {
                card.reset_wake_streak();
            }
            true
        });
    }
}

/// The label of the button or the answer `component_id` of `document`.
fn button_label<'a>(document: &'a BlockDocument, component_id: &str) -> Option<&'a str> {
    document.components.iter().find_map(|component| match component {
        Component::Buttons { items } | Component::Callout { buttons: items, .. } => items
            .iter()
            .find(|button| button.id == component_id)
            .map(|button| button.label.as_str()),
        _ => None,
    })
}

/// The line of the conversation that names one press of the person on a card of
/// an orchestrator. The label and the heading are text of the program, so each is
/// cut to [`PRESS_LINE_CHARS`] characters.
fn press_line(label: &str, heading: &str) -> String {
    format!("You pressed {} on {}", cut_line(label), cut_line(heading))
}

/// One line of at most [`PRESS_LINE_CHARS`] characters, without a control
/// character.
fn cut_line(text: &str) -> String {
    text.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(PRESS_LINE_CHARS)
        .collect()
}
