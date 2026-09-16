//! Firefox CSS viewport application, acknowledgement, and binding recovery.

use serde_json::json;

use crate::session::BrowserEventSender;
use crate::{AgentAction, BrowserControlFailure, BrowserControlValue};

use super::Driver;

impl Driver {
    pub(super) fn begin_resize(&mut self, request: &AgentAction) {
        let result = self.prepare_resize(request);
        match result {
            Ok(pending) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Dispatched);
                self.finish_pending_resize("viewport_superseded", "a newer resize replaced this request");
                self.pending_resize = Some(pending);
            }
            Err(error) => {
                self.audit_agent_action(request, crate::BrowserAuditStatus::Rejected);
                self.complete_agent_action(request, Err(error));
            }
        }
    }

    fn prepare_resize(
        &self,
        request: &AgentAction,
    ) -> Result<crate::session::viewport::PendingResize, BrowserControlFailure> {
        if self.host.is_remote() {
            return Err(BrowserControlFailure::new(
                "remote_viewport_fixed",
                "remote device viewports cannot be resized",
            ));
        }
        if !self.firefox_bidi() {
            return Err(BrowserControlFailure::new(
                "viewport_unsupported",
                "exact viewport resizing requires Chromium or local Firefox BiDi",
            ));
        }
        if self.panel_slot.teach_recording() {
            return Err(BrowserControlFailure::new(
                "teach_recording",
                "Teach mode has exclusive ownership of this panel",
            ));
        }
        crate::session::viewport::PendingResize::new(request, self.viewport_policy)
    }

    pub(super) fn finish_pending_resize(&mut self, code: &str, message: &str) {
        if let Some(pending) = self.pending_resize.take() {
            if let Some(coordination) = &self.config.coordination {
                coordination.retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
            }
            self.complete_agent_action(&pending.request, Err(BrowserControlFailure::new(code, message)));
        }
    }

    pub(super) fn tick_pending_resize(&mut self, events: &BrowserEventSender, stop: &std::sync::atomic::AtomicBool) {
        let Some(mut pending) = self.pending_resize.take() else {
            if !stop.load(std::sync::atomic::Ordering::Acquire) {
                self.restore_viewport_binding(events);
            }
            return;
        };
        match self.advance_resize(&mut pending, events, stop) {
            Ok(None) => self.pending_resize = Some(pending),
            Ok(Some(value)) => self.complete_agent_action(&pending.request, Ok(value)),
            Err(error) => {
                if let Some(coordination) = &self.config.coordination {
                    coordination
                        .retain_action_result_on_remove(&self.config.panel_local_id, &pending.request.action_id);
                }
                self.complete_agent_action(&pending.request, Err(error));
            }
        }
    }

    fn restore_viewport_binding(&mut self, events: &BrowserEventSender) {
        let now = std::time::Instant::now();
        if now < self.viewport_restore_at {
            return;
        }
        self.viewport_restore_at = now + std::time::Duration::from_millis(100);
        let Some(target) = self
            .viewport_policy
            .rebind_target(self.viewport_context.as_deref(), self.context_id.as_deref())
        else {
            return;
        };
        if let Err(error) = self.apply_explicit_viewport(target, std::time::Duration::from_millis(100), events) {
            tracing::debug!("viewport restoration pending: {}", error.message);
        }
    }

    fn apply_explicit_viewport(
        &mut self,
        [width, height]: [u32; 2],
        timeout: std::time::Duration,
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        let context = self.context_id.clone();
        let link = self
            .bidi
            .as_mut()
            .ok_or_else(|| BrowserControlFailure::new("viewport_unsupported", "Firefox BiDi is unavailable"))?;
        let outcome = link.call(
            timeout,
            "browsingContext.setViewport",
            &json!({
                "context": context, "viewport": { "width": width, "height": height },
            }),
        );
        for event in outcome.events {
            self.handle_bidi_event(&event, events);
        }
        outcome
            .result
            .map_err(|error| BrowserControlFailure::new("viewport_failed", error.to_string()))?;
        // Events drained during the command may bind a different context.
        self.viewport_context = context;
        self.advance_generation();
        self.frames.demand();
        Ok(())
    }

    fn advance_resize(
        &mut self,
        pending: &mut crate::session::viewport::PendingResize,
        events: &BrowserEventSender,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<BrowserControlValue>, BrowserControlFailure> {
        pending.guard(
            self.owner_seen.as_deref(),
            self.handoff_seen.is_some(),
            self.panel_slot.teach_recording()
                || self
                    .last_user_active_stamp
                    .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(5)),
            stop.load(std::sync::atomic::Ordering::Acquire) || self.host.has_exited(),
        )?;
        if !pending.poll_due() || self.context_id.is_none() {
            return Ok(None);
        }
        let target = self.viewport_policy.target(pending.requested);
        if pending.target != target {
            pending.bound_session = None;
            pending.target = target;
        }
        if pending.bound_session.is_none() || pending.bound_session != self.context_id {
            let context = self.context_id.clone();
            let result = self.apply_explicit_viewport(target, pending.budget()?, events);
            if result.is_err() && context != self.context_id {
                return Ok(None);
            }
            result?;
            self.viewport_policy.commit(pending.requested);
            self.panel_slot.set_viewport_override(pending.requested);
            pending.bound_session = context;
            return Ok(None);
        }
        let Ok(value) = self.evaluate_json_within(crate::session::viewport::MEASURE_VIEWPORT, Some(pending.budget()?))
        else {
            pending.budget()?;
            return Ok(None);
        };
        let result = pending.observe(value, self.signal_epoch)?;
        self.request_signal_refresh();
        Ok(result)
    }
}
