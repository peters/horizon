//! The waits of a `next_click`: the open wait of one card and of every card of one
//! owner, and the answer that a click, a deadline or a closed card ends.
//!
//! A wait lives in the state of `BlockClickState` and holds the request until its
//! answer, which the coordination worker writes.

use std::path::{Path, PathBuf};

use horizon_browser_control::block::ClickEvent;
use horizon_browser_control::manifest;
use horizon_browser_control::manifest::block::{Operation, Outcome, Request};
use horizon_core::{PanelId, WorkspaceId};

use super::super::HorizonApp;
use super::super::block_requests::{BlockAnswer, card_owner};
use super::host_apply_millis;

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

impl HorizonApp {
    /// Starts a `next_click` wait. A card that already holds a click answers at
    /// once, and so does a caller that must not wait on this card. `None` means
    /// the host keeps the request until a click or the deadline: the request is
    /// still applied on the UI thread, only its answer is deferred.
    ///
    /// `panel_id` names one card. Without it, the wait covers every card that
    /// the caller owns, because an agent makes one tool call at a time: the
    /// oldest click of those cards answers it.
    pub(in crate::app) fn begin_click_wait(
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
        let answer = match panel_id {
            Some(panel_id) => self.begin_card_click_wait(root, request, panel_id),
            None => self.begin_any_card_click_wait(root, request),
        };
        // A wait that stays open makes the buttons of its card a question of the
        // person, which the sidebar shows.
        self.sidebar_list.count_inbox_change();
        answer
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
    /// Answers the waits a click, a deadline or a closed card ends.
    ///
    /// The waits that name a card settle first, so a click reaches the single
    /// card waiter of that card when one exists; a wait that covers every card
    /// of an agent takes what is left.
    pub(super) fn settle_click_waiters(&mut self, now_millis: i64) -> bool {
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
        if answered {
            // A wait that ended asks no longer, so the sidebar reads its questions.
            self.sidebar_list.count_inbox_change();
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
}
