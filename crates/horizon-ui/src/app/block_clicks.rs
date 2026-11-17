//! The clicks a person makes on a card, on their way to the agent that owns it.
//!
//! The widget records a click in the card's runtime state. This module settles
//! it: a `next_click` that waits on the card or on any card of its owner is
//! answered with the click, and a click that nobody waits for wakes its owner
//! with one fixed line in the owner's terminal. Nothing here is saved, and no
//! step touches the disk on the UI thread: an answer is written by the
//! coordination worker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use horizon_browser_control::block::ClickEvent;
use horizon_browser_control::manifest;
use horizon_browser_control::manifest::block::{HOST_APPLY_TIMEOUT, Operation, Outcome, Request};
use horizon_core::node_cards;
use horizon_core::{
    AgentStatus, BlockPanelState, CardRuntime, Panel, PanelId, PanelKind, WAKE_LINE, WakeContext, WakeVerdict,
    WorkspaceId, browser_actor, wake_verdict,
};

use crate::block_widget::{CardChip, chip::ChipState};

use super::HorizonApp;
use super::block_requests::{BlockAnswer, card_owner};

/// A `next_click` that waits until a click arrives or the person's wait ends.
pub(in crate::app) struct ClickWaiter {
    root: Option<PathBuf>,
    request: Request,
    /// Wall-clock time the wait ends, in milliseconds after the epoch.
    deadline_at_millis: i64,
}

impl ClickWaiter {
    fn new(root: Option<&Path>, request: &Request) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            request: request.clone(),
            deadline_at_millis: request.deadline_at_millis.saturating_sub(host_apply_millis()),
        }
    }

    /// Whether the wait ends at or before `now_millis`.
    fn deadline_passed(&self, now_millis: i64) -> bool {
        self.deadline_at_millis <= now_millis
    }
}

/// What this host owes a card's owner: the open `next_click` waits, and the
/// last wake line to each agent panel.
#[derive(Default)]
pub(in crate::app) struct BlockClickState {
    /// The waits that name one card, by that card.
    card_waiters: HashMap<PanelId, ClickWaiter>,
    /// The waits that cover every card of an owner, by that owner's actor.
    agent_waiters: HashMap<String, ClickWaiter>,
    last_wake: HashMap<PanelId, Instant>,
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
        // A folded card keeps the words of the badge of its heading on its one line.
        if let Some((_, badge)) = card.folded_line() {
            let state = ChipState::Present;
            return badge.map(|name| CardChip { name, state });
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

    /// Whether the owner of a card is here: a panel of its workspace, or the
    /// session of a machine that the watch reports. A session whose program is no
    /// agent reports no row, so the list of sessions is what settles it.
    pub(in crate::app) fn card_owner_present(&self, panel: &Panel) -> bool {
        if self.card_owner_panel(panel).is_some() {
            return true;
        }
        let Some((machine, session)) = card_owner(panel).and_then(node_cards::actor_parts) else {
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
    fn take_card_click(&mut self, panel_id: PanelId) -> Option<ClickEvent> {
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
    fn reset_card_wake_streak(&mut self, panel_id: PanelId) {
        if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&panel_id) {
            card.reset_wake_streak();
        }
    }

    /// Starts a `next_click` wait. A card that already holds a click answers at
    /// once, and so does a caller that must not wait on this card. `None` means
    /// the host keeps the request until a click or the deadline: the request is
    /// still applied on the UI thread, only its answer is deferred.
    ///
    /// `panel_id` names one card. Without it, the wait covers every card that
    /// the caller owns, because an agent makes one tool call at a time: the
    /// oldest click of those cards answers it.
    pub(super) fn begin_click_wait(
        &mut self,
        root: Option<&Path>,
        request: &Request,
        panel_id: Option<&str>,
    ) -> Option<Outcome> {
        let Operation::NextClick { .. } = &request.operation else {
            return None;
        };
        if let Err(refusal) = self.request_caller(request) {
            return Some(refusal);
        }
        match panel_id {
            Some(panel_id) => self.begin_card_click_wait(root, request, panel_id),
            None => self.begin_any_card_click_wait(root, request),
        }
    }

    /// Starts a wait that names one card.
    fn begin_card_click_wait(&mut self, root: Option<&Path>, request: &Request, panel_id: &str) -> Option<Outcome> {
        let card = match self.owned_card(panel_id, &request.actor) {
            Ok(card) => card,
            Err(refusal) => return Some(refusal),
        };
        // One wait per card: an older wait ends before this one starts, with the
        // click that already waits, or as a timeout.
        if let Some(older) = self.block_clicks.card_waiters.remove(&card) {
            let outcome = self
                .take_card_click(card)
                .map_or(Outcome::TimedOut { timed_out: true }, |click| Outcome::Clicked {
                    click,
                });
            self.answer_waiter(older, outcome);
        }
        if let Some(click) = self.take_card_click(card) {
            return Some(Outcome::Clicked { click });
        }
        self.reset_card_wake_streak(card);
        self.block_clicks
            .card_waiters
            .insert(card, ClickWaiter::new(root, request));
        None
    }

    /// Starts a wait that covers every card of the caller. A caller that owns
    /// no card learns that at once, and a second wait of one agent ends the
    /// older one first: one agent waits once.
    fn begin_any_card_click_wait(&mut self, root: Option<&Path>, request: &Request) -> Option<Outcome> {
        let cards = self
            .owner_workspace(&request.actor)
            .map_or_else(Vec::new, |workspace| self.owned_card_ids(workspace, &request.actor));
        if cards.is_empty() {
            return Some(Outcome::failed(
                "no_cards",
                "This agent owns no card in its workspace yet; create one first",
            ));
        }
        if let Some(older) = self.block_clicks.agent_waiters.remove(&request.actor) {
            self.answer_waiter(older, Outcome::TimedOut { timed_out: true });
        }
        if let Some((card, click)) = self.oldest_card_click(&cards) {
            self.reset_card_wake_streak(card);
            return Some(Outcome::Clicked { click });
        }
        for card in cards {
            self.reset_card_wake_streak(card);
        }
        self.block_clicks
            .agent_waiters
            .insert(request.actor.clone(), ClickWaiter::new(root, request));
        None
    }

    /// The cards of one workspace that `owner` created, in board order.
    fn owned_card_ids(&self, workspace: WorkspaceId, owner: &str) -> Vec<PanelId> {
        self.workspace_cards(workspace)
            .iter()
            .filter(|panel| card_owner(panel) == Some(owner))
            .map(|panel| panel.id)
            .collect()
    }

    /// The oldest click that waits on any of `cards`, with its card. Cards are
    /// visited in board order, so two clicks of the same millisecond keep that
    /// order.
    fn oldest_card_click(&mut self, cards: &[PanelId]) -> Option<(PanelId, ClickEvent)> {
        let now_millis = manifest::now_millis();
        let mut oldest: Option<(PanelId, i64)> = None;
        for card in cards {
            let Some(runtime) = self.panel_render_caches.card_runtime.get_mut(card) else {
                continue;
            };
            let Some(at_millis) = runtime.oldest_click_at(now_millis) else {
                continue;
            };
            if oldest.is_none_or(|(_, at)| at_millis < at) {
                oldest = Some((*card, at_millis));
            }
        }
        let (card, _) = oldest?;
        self.take_card_click(card).map(|click| (card, click))
    }

    /// Answers each wait that has a click, a passed deadline or no card left,
    /// drops the clicks that aged out, and wakes an owner when a click waits and
    /// nobody listens. Runs on every frame and on every request pump wake.
    pub(super) fn settle_block_clicks(&mut self) -> bool {
        let now_millis = manifest::now_millis();
        let answered = self.settle_card_answers();
        self.prune_card_clicks(now_millis);
        answered || self.settle_click_waiters(now_millis) || self.wake_card_owners()
    }

    /// Carries the answer that the person pressed to the saved card, and the
    /// answer of a restored card back into the live one.
    ///
    /// A press belongs to the frame in which the person made it, so the widget
    /// records it in the runtime state of the card. A restart has no runtime
    /// state, so the answer is written to the saved card, which the sidebar reads
    /// after a restart, and a card that comes back with an answered callout
    /// starts quiet. Returns whether it saved an answer.
    fn settle_card_answers(&mut self) -> bool {
        let ids = self.block_card_ids();
        let mut saved = false;
        for id in ids {
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

    /// The cards of the board, in board order.
    fn block_card_ids(&self) -> Vec<PanelId> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::Block)
            .map(|panel| panel.id)
            .collect()
    }

    /// Answers the waits a click, a deadline or a closed card ends.
    ///
    /// The waits that name a card settle first, so a click reaches the single
    /// card waiter of that card when one exists; a wait that covers every card
    /// of an agent takes what is left.
    fn settle_click_waiters(&mut self, now_millis: i64) -> bool {
        let waiting: Vec<PanelId> = self.block_clicks.card_waiters.keys().copied().collect();
        let mut answered = false;
        for panel_id in waiting {
            if self.board.panel(panel_id).is_none() {
                answered |= self.close_card_click_wait(panel_id);
            } else if let Some(click) = self.take_card_click(panel_id) {
                answered |= self.answer_card_click(panel_id, Outcome::Clicked { click });
            } else if self.card_wait_deadline_passed(panel_id, now_millis) {
                answered |= self.answer_card_click(panel_id, Outcome::TimedOut { timed_out: true });
            }
        }
        let owners: Vec<String> = self.block_clicks.agent_waiters.keys().cloned().collect();
        for owner in owners {
            answered |= self.settle_agent_wait(&owner, now_millis);
        }
        answered
    }

    /// Answers one wait that covers every card of `owner`.
    fn settle_agent_wait(&mut self, owner: &str, now_millis: i64) -> bool {
        let Some(workspace) = self.owner_workspace(owner) else {
            let Some(waiter) = self.block_clicks.agent_waiters.remove(owner) else {
                return false;
            };
            self.answer_waiter(
                waiter,
                Outcome::failed(
                    "caller_unavailable",
                    "The calling agent panel is no longer in this host",
                ),
            );
            return true;
        };
        let cards = self.owned_card_ids(workspace, owner);
        let Some(waiter) = self.block_clicks.agent_waiters.remove(owner) else {
            return false;
        };
        if cards.is_empty() {
            self.answer_waiter(
                waiter,
                Outcome::failed("panel_unavailable", "The card is no longer in the workspace"),
            );
            return true;
        }
        if let Some((card, click)) = self.oldest_card_click(&cards) {
            self.reset_card_wake_streak(card);
            self.answer_waiter(waiter, Outcome::Clicked { click });
            return true;
        }
        if waiter.deadline_passed(now_millis) {
            self.answer_waiter(waiter, Outcome::TimedOut { timed_out: true });
            return true;
        }
        // Nothing happened yet: the wait stays open.
        self.block_clicks.agent_waiters.insert(owner.to_owned(), waiter);
        false
    }

    /// Whether a card's own wait ends at or before `now_millis`.
    fn card_wait_deadline_passed(&self, panel_id: PanelId, now_millis: i64) -> bool {
        self.block_clicks
            .card_waiters
            .get(&panel_id)
            .is_some_and(|waiter| waiter.deadline_passed(now_millis))
    }

    /// Answers the wait on a card that is gone with `panel_unavailable`, and
    /// forgets the card's runtime state.
    fn close_card_click_wait(&mut self, panel_id: PanelId) -> bool {
        self.panel_render_caches.card_runtime.remove(&panel_id);
        let Some(waiter) = self.block_clicks.card_waiters.remove(&panel_id) else {
            return false;
        };
        self.answer_waiter(
            waiter,
            Outcome::failed("panel_unavailable", "The card is no longer in the workspace"),
        );
        true
    }

    /// Answers the wait on a card with one outcome, and starts a new wake
    /// streak: an agent that waited for a click listens again.
    fn answer_card_click(&mut self, panel_id: PanelId, outcome: Outcome) -> bool {
        let Some(waiter) = self.block_clicks.card_waiters.remove(&panel_id) else {
            return false;
        };
        self.reset_card_wake_streak(panel_id);
        self.answer_waiter(waiter, outcome);
        true
    }

    /// Writes the answer of a held wait on the coordination worker.
    fn answer_waiter(&mut self, waiter: ClickWaiter, outcome: Outcome) {
        self.write_block_answer(BlockAnswer::new(waiter.root.as_deref(), waiter.request, outcome));
    }

    /// Drops the clicks that aged out, starts a new wake streak for a card that
    /// holds none, and forgets the cards that left the board.
    ///
    /// The runtime state of a card that is still on the board stays in the map:
    /// the widget inserts it again on the next frame, so dropping it there would
    /// churn the render path.
    fn prune_card_clicks(&mut self, now_millis: i64) {
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

    /// Types the wake line into an idle owner's terminal when a click waits and
    /// nobody listens. A wake that a condition holds is tried again on a later
    /// frame.
    fn wake_card_owners(&mut self) -> bool {
        let wakes = self.planned_wakes();
        let now = Instant::now();
        let mut woken: Vec<PanelId> = Vec::new();
        for (panel_id, owner) in wakes {
            // One line per agent panel: its cards keep their streak for a later
            // frame, behind the interval of the line.
            if woken.contains(&owner) {
                continue;
            }
            if !self.send_wake_line(owner) {
                continue;
            }
            self.block_clicks.last_wake.insert(owner, now);
            if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&panel_id) {
                card.mark_wake_sent();
            }
            woken.push(owner);
        }
        !woken.is_empty()
    }

    /// The cards whose owner the wake line should reach this frame, with that
    /// owner's agent panel.
    fn planned_wakes(&self) -> Vec<(PanelId, PanelId)> {
        let mut wakes = Vec::new();
        for (panel_id, pending, wake_sent) in self.card_click_facts() {
            if pending == 0 || self.block_listening(panel_id) {
                continue;
            }
            let Some(panel) = self.board.panel(panel_id) else {
                continue;
            };
            let owner = self.card_owner_panel(panel);
            if wake_verdict(&self.wake_context(owner, pending, wake_sent)) == WakeVerdict::Send
                && let Some(owner) = owner
            {
                wakes.push((panel_id, owner));
            }
        }
        wakes
    }

    /// The clicks and wake streak of each card, in board-independent order.
    fn card_click_facts(&self) -> Vec<(PanelId, usize, bool)> {
        let mut facts: Vec<(PanelId, usize, bool)> = self
            .panel_render_caches
            .card_runtime
            .iter()
            .map(|(panel_id, card)| (*panel_id, card.queued(), card.wake_sent()))
            .collect();
        facts.sort_by_key(|(panel_id, _, _)| panel_id.0);
        facts
    }

    /// What the wake decision of one card needs, from the board and the last
    /// wake line to its owner.
    fn wake_context(&self, owner: Option<PanelId>, queued: usize, wake_sent: bool) -> WakeContext {
        let owner_panel = owner.and_then(|owner| self.board.panel(owner));
        WakeContext {
            queued_clicks: queued,
            listening: false,
            owner_present: owner_panel.is_some(),
            owner_focused: owner.is_some_and(|owner| self.board.focused == Some(owner)),
            owner_status: owner_panel.map_or(AgentStatus::Idle, Panel::agent_status),
            wake_already_sent: wake_sent,
            since_last_wake: owner
                .and_then(|owner| self.block_clicks.last_wake.get(&owner))
                .map(Instant::elapsed),
        }
    }

    /// Types the one wake line into an agent panel's terminal, encoded as a
    /// person's paste is, then Enter.
    fn send_wake_line(&mut self, owner: PanelId) -> bool {
        let Some(panel) = self.board.panel_mut(owner) else {
            return false;
        };
        let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
            return false;
        };
        let mut bytes = crate::input::paste_bytes(WAKE_LINE, mode, true);
        bytes.push(b'\r');
        panel.write_input(&bytes);
        true
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
