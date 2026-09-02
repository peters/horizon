//! Agent navigation actions on the Chromium driver: dispatch immediately,
//! settle the typed outcome from page events or the bounded deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::frames::FrameSlot;
use crate::navigation::{AgentActionExecution, NavigationSignal, PendingNavigation};
use crate::{AgentAction, DEFAULT_NAVIGATION_TIMEOUT_MILLIS, NavigationWait, normalize_navigation_target};

use super::{BrowserEventSender, DriverState};

impl DriverState {
    /// Dispatch `Page.navigate` for a `Navigate` agent action. A dispatch-only
    /// wait settles at once; otherwise the action stays pending until the
    /// commit, `DOMContentLoaded`, failure, or deadline arrives.
    pub(super) fn begin_agent_navigation(
        &mut self,
        link: &mut crate::cdp::CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        request: &AgentAction,
    ) -> AgentActionExecution {
        let crate::BrowserControlAction::Navigate {
            url,
            wait,
            timeout_millis,
        } = &request.action
        else {
            return AgentActionExecution::Done(Err(crate::BrowserControlFailure::new(
                "invalid_action_state",
                "navigation was requested for a non-navigation action",
            )));
        };
        let (wait, timeout_millis) = (*wait, *timeout_millis);
        let now = Instant::now();
        let pending = PendingNavigation::new(
            request.clone(),
            normalize_navigation_target(url),
            wait,
            Duration::from_millis(timeout_millis.unwrap_or(DEFAULT_NAVIGATION_TIMEOUT_MILLIS)),
            PendingNavigation::queued_for(request, crate::navigation::now_millis()),
            now,
        );
        if let Some(expired) = pending.tick(now) {
            // The bound elapsed while the action sat in the queue: report it
            // without touching the page after the caller's deadline, and
            // without superseding a navigation that keeps running unreplaced.
            return AgentActionExecution::Done(expired);
        }
        self.supersede_pending_navigation(now);
        self.interaction_started_at.get_or_insert(now);
        self.vertical_scrollbar_drag = None;
        self.invalidate_scrollbar_layout();
        if let Err(failure) = self.navigate_to(link, event_tx, frame_slot, url) {
            return AgentActionExecution::Done(Err(failure));
        }
        if wait == NavigationWait::Dispatched {
            return AgentActionExecution::Done(Ok(pending.dispatched(Instant::now())));
        }
        tracing::debug!(
            target: "browser",
            action_id = %request.action_id,
            ?wait,
            timeout_millis,
            "agent navigation pending"
        );
        self.pending_navigation = Some(pending);
        AgentActionExecution::Pending
    }

    /// The `Page.navigate` reply named the navigation's loader (or none for a
    /// same-document change); attribute any held-back commit.
    pub(super) fn attach_navigation_id(&mut self, loader_id: Option<&str>) {
        let now = Instant::now();
        let settled = self
            .pending_navigation
            .as_mut()
            .and_then(|pending| pending.attach_id(loader_id, now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            tracing::debug!(target: "browser", action_id = %pending.request.action_id, "agent navigation settled on dispatch reply");
            self.complete_agent_action(&pending.request, result);
        }
    }

    /// Feed a page signal to the pending navigation and complete the action
    /// once it settles.
    pub(super) fn observe_navigation_signal(&mut self, signal: NavigationSignal<'_>) {
        let now = Instant::now();
        let settled = self
            .pending_navigation
            .as_mut()
            .and_then(|pending| pending.observe(signal, now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            tracing::debug!(target: "browser", action_id = %pending.request.action_id, ?signal, "agent navigation settled");
            self.complete_agent_action(&pending.request, result);
        }
    }

    /// Report a timed-out navigation with the latest page state.
    pub(super) fn tick_pending_navigation(&mut self) {
        let now = Instant::now();
        let settled = self.pending_navigation.as_ref().and_then(|pending| pending.tick(now));
        if let Some(result) = settled
            && let Some(pending) = self.pending_navigation.take()
        {
            tracing::debug!(target: "browser", action_id = %pending.request.action_id, "agent navigation timed out");
            self.complete_agent_action(&pending.request, result);
        }
    }

    pub(super) fn supersede_pending_navigation(&mut self, now: Instant) {
        // Dispatch-only and timed-out actions can leave the asynchronous reply
        // in flight after their logical result is gone. A replacement must not
        // route that stale reply into its own navigation state.
        self.navigate_request_id = None;
        if let Some(pending) = self.pending_navigation.take() {
            self.complete_agent_action(&pending.request, Ok(pending.superseded(now)));
        }
    }
}
