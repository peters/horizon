//! The clicks a person makes on a card, on their way to the agent that owns it.
//!
//! The widget records a click in the card's runtime state. This module settles
//! it: a `next_click` that waits on the card or on any card of its owner is
//! answered with the click, and a click that nobody waits for wakes its owner
//! with one fixed line in the owner's terminal. Nothing here is saved, and no
//! step touches the disk on the UI thread: an answer is written by the
//! coordination worker.

use std::collections::HashMap;
use std::time::Instant;

use horizon_browser_control::manifest;
use horizon_browser_control::manifest::block::HOST_APPLY_TIMEOUT;
use horizon_core::PanelId;

use super::HorizonApp;

mod press;
mod waits;
mod wake;

pub(in crate::app) use self::wake::WakeTarget;
use waits::ClickWaiter;

/// What this host owes a card's owner: the open `next_click` waits, and the
/// last wake of each owner.
#[derive(Default)]
pub(in crate::app) struct BlockClickState {
    /// The waits that name one card, by that card.
    card_waiters: HashMap<PanelId, ClickWaiter>,
    /// The waits that cover every card of an owner, by that owner's actor.
    agent_waiters: HashMap<String, ClickWaiter>,
    last_wake: HashMap<WakeTarget, Instant>,
    /// The cards of the board and the revision of each, as the last frame saw
    /// them. A card that comes, goes or holds another document changes the
    /// questions and the words of the sidebar, which then reads them again.
    cards: Vec<(PanelId, u64)>,
}

impl BlockClickState {
    /// Whether a `next_click` waits on `panel_id` alone.
    fn listening_on_card(&self, panel_id: PanelId) -> bool {
        self.card_waiters.contains_key(&panel_id)
    }

    /// Whether a `next_click` of `owner` waits on any of its cards.
    fn listening_on_owner(&self, owner: &str) -> bool {
        self.agent_waiters.contains_key(owner)
    }
}

impl HorizonApp {
    /// Answers each wait that has a click, a passed deadline or no card left,
    /// names the press of each card in the conversation of an orchestrator, drops
    /// the clicks that aged out, and wakes an owner when a click waits and nobody
    /// listens. Runs on every frame and on every request pump wake.
    pub(super) fn settle_block_clicks(&mut self) -> bool {
        let now_millis = manifest::now_millis();
        let answered = self.settle_card_answers();
        self.prune_card_clicks(now_millis);
        let noted = self.note_card_presses();
        let waited = self.settle_click_waiters(now_millis);
        let woken = self.wake_card_owners();
        answered || noted || waited || woken
    }

    /// Whether a `next_click` or a card of Horizon that asks about a setting
    /// holds this host open. The request pump wakes slowly while one does, so a
    /// deadline passes also when no frame runs.
    pub(in crate::app) fn block_waiters_outstanding(&self) -> bool {
        !self.block_clicks.card_waiters.is_empty()
            || !self.block_clicks.agent_waiters.is_empty()
            || self.settings_request.is_waiting()
    }
}

/// The window the host has to apply a card request, in milliseconds.
fn host_apply_millis() -> i64 {
    i64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
