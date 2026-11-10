//! The clicks a person makes on a card, on their way to the agent that owns it.
//!
//! The widget records a click in the card's runtime state. This module settles
//! it: a `next_click` that waits on the card is answered with the click, and a
//! click that nobody waits for wakes its owner with one fixed line in the
//! owner's terminal. Nothing here is saved, and no step touches the disk on the
//! UI thread: an answer is written by the coordination worker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use horizon_browser_control::block::ClickEvent;
use horizon_browser_control::manifest;
use horizon_browser_control::manifest::block::{HOST_APPLY_TIMEOUT, Operation, Outcome, Request};
use horizon_core::{AgentStatus, Panel, PanelId, WAKE_LINE, WakeContext, WakeVerdict, browser_actor, wake_verdict};

use super::HorizonApp;
use super::block_requests::BlockAnswer;

/// A `next_click` that waits on one card until a click arrives or the person's
/// wait ends.
pub(in crate::app) struct ClickWaiter {
    root: Option<PathBuf>,
    request: Request,
    /// Wall-clock time the wait ends, in milliseconds after the epoch.
    deadline_at_millis: i64,
}

/// What this host owes a card's owner: the open `next_click` waits, and the
/// last wake line to each agent panel.
#[derive(Default)]
pub(in crate::app) struct BlockClickState {
    waiters: HashMap<PanelId, ClickWaiter>,
    last_wake: HashMap<PanelId, Instant>,
}

impl BlockClickState {
    /// Whether a `next_click` waits on `panel_id`, so a click reaches its agent
    /// at once.
    fn listening(&self, panel_id: PanelId) -> bool {
        self.waiters.contains_key(&panel_id)
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

    /// Whether an agent panel of a card's owner is in its workspace, so its
    /// buttons and rows are live.
    pub(in crate::app) fn card_agent_present(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .is_some_and(|panel| self.card_owner_panel(panel).is_some())
    }

    /// Whether a `next_click` waits on a card.
    pub(in crate::app) fn block_listening(&self, panel_id: PanelId) -> bool {
        self.block_clicks.listening(panel_id)
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
    pub(super) fn begin_click_wait(
        &mut self,
        root: Option<&Path>,
        request: &Request,
        panel_id: &str,
    ) -> Option<Outcome> {
        let Operation::NextClick { .. } = &request.operation else {
            return None;
        };
        let card = match self
            .request_caller(request)
            .and_then(|caller| self.owned_card(panel_id, caller, &request.actor))
        {
            Ok(card) => card,
            Err(refusal) => return Some(refusal),
        };
        // One wait per card: an older wait ends before this one starts, with the
        // click that already waits, or as a timeout.
        if let Some(older) = self.block_clicks.waiters.remove(&card) {
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
        self.block_clicks.waiters.insert(
            card,
            ClickWaiter {
                root: root.map(Path::to_path_buf),
                request: request.clone(),
                deadline_at_millis: request.deadline_at_millis.saturating_sub(host_apply_millis()),
            },
        );
        None
    }

    /// Answers each wait that has a click, a passed deadline or no card left,
    /// drops the clicks that aged out, and wakes an owner when a click waits and
    /// nobody listens. Runs on every frame and on every request pump wake.
    pub(super) fn settle_block_clicks(&mut self) -> bool {
        let now_millis = manifest::now_millis();
        self.prune_card_clicks(now_millis);
        self.settle_click_waiters(now_millis) || self.wake_card_owners()
    }

    /// Answers the waits a click, a deadline or a closed card ends.
    fn settle_click_waiters(&mut self, now_millis: i64) -> bool {
        let waiting: Vec<PanelId> = self.block_clicks.waiters.keys().copied().collect();
        let mut answered = false;
        for panel_id in waiting {
            if self.board.panel(panel_id).is_none() {
                answered |= self.close_click_wait(panel_id);
            } else if let Some(click) = self.take_card_click(panel_id) {
                answered |= self.answer_click(panel_id, Outcome::Clicked { click });
            } else if self.wait_deadline_passed(panel_id, now_millis) {
                answered |= self.answer_click(panel_id, Outcome::TimedOut { timed_out: true });
            }
        }
        answered
    }

    /// Whether a card's wait ends at or before `now_millis`.
    fn wait_deadline_passed(&self, panel_id: PanelId, now_millis: i64) -> bool {
        self.block_clicks
            .waiters
            .get(&panel_id)
            .is_some_and(|waiter| waiter.deadline_at_millis <= now_millis)
    }

    /// Answers the wait on a card that is gone with `panel_unavailable`, and
    /// forgets the card's runtime state.
    fn close_click_wait(&mut self, panel_id: PanelId) -> bool {
        self.panel_render_caches.card_runtime.remove(&panel_id);
        let Some(waiter) = self.block_clicks.waiters.remove(&panel_id) else {
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
    fn answer_click(&mut self, panel_id: PanelId, outcome: Outcome) -> bool {
        let Some(waiter) = self.block_clicks.waiters.remove(&panel_id) else {
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
            if pending == 0 || self.block_clicks.listening(panel_id) {
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

    /// Whether a `next_click` holds this host open. The request pump wakes
    /// slowly while one does, so a deadline passes also when no frame runs.
    pub(in crate::app) fn block_waiters_outstanding(&self) -> bool {
        !self.block_clicks.waiters.is_empty()
    }
}

/// The window the host has to apply a card request, in milliseconds.
fn host_apply_millis() -> i64 {
    i64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
