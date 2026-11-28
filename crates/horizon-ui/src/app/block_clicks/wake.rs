//! The wake of the owner of a card: the one line that tells it that a press
//! waits, and which of its two homes that line goes to.
//!
//! An agent panel reads the line in its terminal. An orchestrator has no
//! terminal, so the same line goes to the program of the dock as one turn of the
//! conversation of its workspace. The decision is the rule of
//! `horizon_core::block_panel::wake`: this module works out what a wake can
//! reach and sends it, so no condition of that rule is copied here.

use std::time::Instant;

use horizon_core::agent_bridge::actor;
use horizon_core::{AgentStatus, Panel, PanelId, WAKE_LINE, WakeContext, WakeVerdict, wake_verdict};

use super::super::HorizonApp;
use super::super::block_requests::card_owner;

/// What a wake reaches: the terminal of an agent panel that owns a card, or the
/// program that the dock runs for the workspace of an orchestrator. One owner
/// gets at most one wake for one streak of presses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum WakeTarget {
    /// The agent panel of the owner, which reads the wake line in its terminal.
    Panel(PanelId),
    /// The program of the dock of that workspace, which reads the wake line as a
    /// turn of its conversation.
    Orchestrator(String),
}
impl HorizonApp {
    /// Wakes the owner of each card whose press nobody reads this frame. A wake
    /// that a condition holds is tried again on a later frame.
    pub(super) fn wake_card_owners(&mut self) -> bool {
        let wakes = self.planned_wakes();
        let now = Instant::now();
        let mut woken: Vec<WakeTarget> = Vec::new();
        for (panel_id, target) in wakes {
            // One wake per owner: its cards keep their streak for a later frame,
            // behind the interval of the line.
            if woken.contains(&target) {
                continue;
            }
            if !self.send_wake(&target) {
                continue;
            }
            self.block_clicks.last_wake.insert(target.clone(), now);
            if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&panel_id) {
                card.mark_wake_sent();
            }
            woken.push(target);
        }
        !woken.is_empty()
    }

    /// The cards whose owner a wake should reach this frame, with what that wake
    /// reaches.
    pub(super) fn planned_wakes(&self) -> Vec<(PanelId, WakeTarget)> {
        let mut wakes = Vec::new();
        for (panel_id, pending, wake_sent) in self.card_click_facts() {
            if pending == 0 || self.block_listening(panel_id) {
                continue;
            }
            let Some(panel) = self.board.panel(panel_id) else {
                continue;
            };
            let target = self.wake_target(panel);
            if wake_verdict(&self.wake_context(target.as_ref(), pending, wake_sent)) == WakeVerdict::Send
                && let Some(target) = target
            {
                wakes.push((panel_id, target));
            }
        }
        wakes
    }

    /// What a wake can reach for the owner of `panel`, while that owner is here:
    /// the terminal of an agent panel, or the program that the dock runs for the
    /// workspace of an orchestrator. The dock wakes only the program that owns
    /// the workspace of the card.
    fn wake_target(&self, panel: &Panel) -> Option<WakeTarget> {
        if let Some(owner) = self.card_owner_panel(panel) {
            return Some(WakeTarget::Panel(owner));
        }
        let local_id = card_owner(panel).and_then(actor::workspace_of)?;
        (self.board.workspace_id_by_local_id(local_id) == Some(panel.workspace_id)
            && self.orchestrator_dock.running(local_id))
        .then(|| WakeTarget::Orchestrator(local_id.to_owned()))
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

    /// What the wake decision of one card needs, from the board, the conversation
    /// of an orchestrator and the last wake of that owner.
    fn wake_context(&self, target: Option<&WakeTarget>, queued: usize, wake_sent: bool) -> WakeContext {
        let owner = match target {
            Some(WakeTarget::Panel(owner)) => Some(*owner),
            _ => None,
        };
        let owner_panel = owner.and_then(|owner| self.board.panel(owner));
        // An orchestrator has no terminal and no panel of this board, so the
        // person cannot be typing in it, and the turn of its conversation is what
        // holds a second wake.
        let owner_status = match target {
            Some(WakeTarget::Orchestrator(local_id)) if self.orchestrator_dock.busy(local_id) => AgentStatus::Working,
            Some(WakeTarget::Orchestrator(_)) => AgentStatus::Idle,
            _ => owner_panel.map_or(AgentStatus::Idle, Panel::agent_status),
        };
        WakeContext {
            queued_clicks: queued,
            listening: false,
            owner_present: target.is_some(),
            owner_focused: owner.is_some_and(|owner| self.board.focused == Some(owner)),
            owner_status,
            wake_already_sent: wake_sent,
            since_last_wake: target
                .and_then(|target| self.block_clicks.last_wake.get(target))
                .map(Instant::elapsed),
        }
    }

    /// The one wake line of this host, on its way to the owner of a card: typed
    /// into the terminal of an agent panel, or sent to the program of the dock as
    /// one turn of the conversation of its workspace.
    fn send_wake(&mut self, target: &WakeTarget) -> bool {
        match target {
            WakeTarget::Panel(owner) => self.send_wake_line(*owner),
            WakeTarget::Orchestrator(local_id) => self.orchestrator_dock.wake(local_id),
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
}
