//! Selector waits on the Chromium driver: one audited action observed from
//! the driver loop at a bounded cadence instead of repeated agent queries.

use std::sync::Arc;
use std::time::Instant;

use crate::frames::FrameSlot;
use crate::navigation::{AgentActionExecution, PendingNavigation, now_millis};
use crate::process::ChromeProcess;
use crate::wait::{
    BackendChecked, Observation, PendingWait, WAIT_MAX_RESULTS, WaitResult, WaitStop, defer_result_during_shutdown,
    run_while_backend_available,
};
use crate::{AgentAction, BrowserControlAction, BrowserControlFailure};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    /// Start a `WaitForSelector` action: observe once now, then keep it
    /// pending until the condition, the bound, or a cancellation.
    pub(super) fn begin_agent_wait(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        request: &AgentAction,
        chrome: &mut ChromeProcess,
    ) -> AgentActionExecution {
        let BrowserControlAction::WaitForSelector {
            selector,
            state,
            timeout_millis,
        } = &request.action
        else {
            return AgentActionExecution::Done(Err(BrowserControlFailure::new(
                "invalid_action_state",
                "a wait was requested for a non-wait action",
            )));
        };
        let now = Instant::now();
        let mut pending = PendingWait::new(
            request.clone(),
            selector.clone(),
            *state,
            *timeout_millis,
            PendingNavigation::queued_for(request, now_millis()),
            self.semantic.generation(),
            now,
        );
        if let Some(expired) = pending.tick(self.semantic.generation(), now) {
            // The bound elapsed while the action sat in the queue: report it
            // without superseding a wait that is still valid.
            return AgentActionExecution::Done(expired);
        }
        self.supersede_pending_wait(now);
        // The first observation runs under the same guards as every later one:
        // an already-expired bound, a lost lease, a pending handoff, or an
        // uncommitted navigation must not let it succeed against the old page.
        let result = {
            let mut backend = (&mut *self, &mut *chrome);
            run_while_backend_available(
                &mut backend,
                |(_, chrome)| chrome.child_status().is_some(),
                |(state, _)| state.advance_wait(link, event_tx, frame_slot, &mut pending, now, true),
            )
        };
        let result = match result {
            BackendChecked::Available(result) => result,
            BackendChecked::Unavailable => {
                return AgentActionExecution::Done(self.retained_shutdown_wait_result(&pending, Instant::now()));
            }
        };
        if let Some(result) = defer_result_during_shutdown(&self.stop_requested, result) {
            return AgentActionExecution::Done(result);
        }
        tracing::debug!(target: "browser", action_id = %request.action_id, ?state, timeout_millis, "agent wait pending");
        self.pending_wait = Some(pending);
        AgentActionExecution::Pending
    }

    /// Advance the pending wait: cancel on a handoff, a lost lease, or a new
    /// page generation; time out at the bound; otherwise observe when due.
    pub(super) fn tick_pending_wait(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        chrome: &mut ChromeProcess,
    ) {
        let Some(mut pending) = self.pending_wait.take() else {
            return;
        };
        let now = Instant::now();
        let observe = pending.poll_due(now);
        let result = {
            let mut backend = (&mut *self, &mut *chrome);
            run_while_backend_available(
                &mut backend,
                |(_, chrome)| chrome.child_status().is_some(),
                |(state, _)| state.advance_wait(link, event_tx, frame_slot, &mut pending, now, observe),
            )
        };
        let result = match result {
            BackendChecked::Available(result) => result,
            BackendChecked::Unavailable => {
                self.complete_wait_for_shutdown(&pending, Instant::now());
                return;
            }
        };
        let result = defer_result_during_shutdown(&self.stop_requested, result);
        match result {
            Some(result) => {
                tracing::debug!(target: "browser", action_id = %pending.request.action_id, ok = result.is_ok(), "agent wait settled");
                self.complete_agent_action(&pending.request, result);
            }
            None => self.pending_wait = Some(pending),
        }
    }

    /// Cancel on a handoff or lost lease, time out or invalidate at the bound
    /// or a new page generation, otherwise observe when asked and no agent
    /// navigation is still uncommitted.
    fn advance_wait(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        pending: &mut PendingWait,
        now: Instant,
        observe: bool,
    ) -> Option<WaitResult> {
        if self.handoff_seen.is_some() {
            return Some(pending.stopped(WaitStop::HandoffPending, now));
        }
        if self.owner_seen.as_deref() != Some(pending.request.actor.as_str()) {
            return Some(pending.stopped(WaitStop::OwnershipLost, now));
        }
        if let Some(result) = pending.tick(self.semantic.generation(), now) {
            return Some(result);
        }
        if self.navigation_in_flight() {
            // The document is changing under this wait; it can only be judged
            // against the new one, which is not what it was asked about.
            pending.block_for_navigation();
            return None;
        }
        if pending.blocked_by_navigation() {
            return Some(pending.stopped(WaitStop::NavigationInvalidated, now));
        }
        // A satisfied observation completes only after the signals have been
        // re-read, events drained, and no navigation started meanwhile: the
        // guards above ran again first.
        if let Some(scan) = pending.take_deferred(self.signal_epoch) {
            // Register the released scan now, so the returned references are
            // the current ones and no earlier observation disturbed the map.
            return Some(match self.semantic_register_scan(scan) {
                Ok((generation, revision, nodes)) => Ok(pending.outcome(generation, revision, nodes, now)),
                Err(failure) => Err(failure),
            });
        }
        if observe {
            return self.observe_wait(link, event_tx, frame_slot, pending, now);
        }
        None
    }

    /// Whether a document is still uncommitted: a pending agent navigation,
    /// or any navigation the driver started (agent, user, or timed-out
    /// action) whose commit or failure has not arrived yet.
    fn navigation_in_flight(&self) -> bool {
        self.pending_navigation.is_some()
            || self.top_frame_navigating
            || (self.retain_frame_during_navigation && !self.navigation_failed)
    }

    fn observe_wait(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        pending: &mut PendingWait,
        now: Instant,
    ) -> Option<WaitResult> {
        // The evaluation may block; give it no more than the remaining bound
        // and judge the result against a fresh clock afterwards. The scan is
        // peeked, not registered: only the released scan becomes references.
        let budget = pending.observation_budget(now);
        let observed =
            self.semantic_peek_within(link, event_tx, frame_slot, &pending.selector, WAIT_MAX_RESULTS, budget);
        let now = Instant::now();
        match observed {
            Ok((generation, nodes, summary, scan)) => match pending.observe(generation, &nodes, summary, now) {
                Observation::Satisfied => {
                    pending.defer(scan, self.signal_epoch);
                    self.request_signal_refresh();
                    None
                }
                Observation::Waiting => None,
                Observation::Done(result) => Some(result),
            },
            Err(failure) => pending.observe_failure(failure, now),
        }
    }

    fn supersede_pending_wait(&mut self, now: Instant) {
        if let Some(pending) = self.pending_wait.take() {
            self.complete_agent_action(&pending.request, pending.stopped(WaitStop::Superseded, now));
        }
    }

    /// Publish a terminal result before the driver loop releases its state.
    pub(super) fn settle_pending_wait_for_shutdown(&mut self, now: Instant) {
        if let Some(pending) = self.pending_wait.take() {
            self.complete_wait_for_shutdown(&pending, now);
        }
    }

    fn complete_wait_for_shutdown(&mut self, pending: &PendingWait, now: Instant) {
        let result = self.retained_shutdown_wait_result(pending, now);
        self.complete_agent_action(&pending.request, result);
    }

    fn retained_shutdown_wait_result(&self, pending: &PendingWait, now: Instant) -> WaitResult {
        if let Some(coordination) = self.config.coordination.as_ref() {
            coordination.retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
        }
        pending.stopped(WaitStop::BrowserUnavailable, now)
    }
}
