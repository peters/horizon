//! Rotation requests observed from the servicing loop, with current ownership guards.
use super::super::orientation::{observe, remaining, set};
use super::Driver;
mod origin;
use crate::remote::{OrientationSupport, RemoteOrientation};
use crate::session::BrowserEventSender;
use crate::{AgentAction, BrowserControlAction, BrowserControlFailure, BrowserControlValue};
use origin::Origin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) struct Pending {
    origin: Origin,
    requested: RemoteOrientation,
    deadline: Instant,
    next_sample: Instant,
    generation: u64,
    verified: Option<([u32; 2], u64)>,
}
impl Driver {
    fn orientation_human_active(&self) -> bool {
        self.panel_slot.teach_recording()
            || self
                .last_user_active_stamp
                .is_some_and(|at| at.elapsed() < super::coordination::USER_ACTIVE_TTL)
    }
    pub(super) fn begin_orientation(&mut self, request: &AgentAction, stopped: &AtomicBool) {
        let prepared = self.prepare_orientation(request, stopped);
        match prepared {
            Ok(pending) => {
                self.orientation_action_id = Some(request.action_id.clone());
                self.orientation_error = None;
                self.audit_agent_action(request, crate::BrowserAuditStatus::Dispatched);
                if let Err(error) = self.dispatch_orientation(pending) {
                    self.orientation_error = Some(format!("{}: {}", error.code, error.message));
                    self.complete_agent_action(request, Err(error));
                }
            }
            Err(error) => {
                if self.pending_orientation.is_none() {
                    self.orientation_error = Some(format!("{}: {}", error.code, error.message));
                }
                self.audit_agent_action(request, crate::BrowserAuditStatus::Rejected);
                self.complete_agent_action(request, Err(error));
            }
        }
    }
    fn prepare_orientation(
        &self,
        request: &AgentAction,
        stopped: &AtomicBool,
    ) -> Result<Pending, BrowserControlFailure> {
        if stopped.load(Ordering::Acquire) {
            return Err(BrowserControlFailure::new(
                "browser_unavailable",
                "the browser is stopping",
            ));
        }
        request
            .action
            .validate()
            .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        if self
            .pending_orientation
            .as_ref()
            .is_some_and(|pending| matches!(pending.origin, Origin::User { .. }))
        {
            return Err(BrowserControlFailure::new(
                "orientation_user_active",
                "a user rotation is still awaiting acknowledgement",
            ));
        }
        if self.owner_seen.as_deref() != Some(request.actor.as_str()) {
            return Err(BrowserControlFailure::new(
                "orientation_ownership_lost",
                "the requesting agent no longer owns this panel",
            ));
        }
        if self.handoff_seen.is_some() {
            return Err(BrowserControlFailure::new(
                "orientation_handoff_pending",
                "the panel is awaiting human steering",
            ));
        }
        let BrowserControlAction::Orientation {
            orientation,
            timeout_millis,
        } = request.action
        else {
            return Err(BrowserControlFailure::new(
                "invalid_action_state",
                "expected orientation action",
            ));
        };
        let state = self.remote_orientation.as_ref().ok_or_else(|| {
            BrowserControlFailure::new("orientation_unsupported", "local browsers use browser_resize")
        })?;
        if state.support == OrientationSupport::Unsupported {
            return Err(BrowserControlFailure::new(
                "orientation_unsupported",
                "this endpoint does not support orientation",
            ));
        }
        if self.orientation_human_active() {
            return Err(BrowserControlFailure::new(
                "orientation_user_active",
                "human input or Teach mode has exclusive ownership",
            ));
        }
        let queued =
            u64::try_from(crate::navigation::now_millis().saturating_sub(request.requested_at_millis)).unwrap_or(0);
        let deadline = Instant::now() + Duration::from_millis(timeout_millis.saturating_sub(queued));
        remaining(deadline)?;
        Ok(Pending {
            origin: Origin::Agent(request.clone()),
            requested: orientation,
            deadline,
            next_sample: Instant::now(),
            generation: self.semantic.generation(),
            verified: None,
        })
    }
    fn dispatch_orientation(&mut self, mut pending: Pending) -> Result<(), BrowserControlFailure> {
        self.finish_orientation(
            "orientation_superseded",
            "a newer rotation superseded this request; inspect applied orientation",
        );
        self.orientation_error = None;
        self.coordination_dirty = true;
        // A POST may mutate even when its response is lost. Discard stale geometry first.
        if let Some(state) = self.remote_orientation.as_mut() {
            state.applied = None;
        }
        self.note_remote_activity();
        self.semantic.invalidate();
        self.advance_viewport_generation();
        self.frames.invalidate();
        let result = set(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            pending.requested,
            pending.deadline,
        );
        if let Err(error) = result {
            if matches!(pending.origin, Origin::User { .. }) {
                self.complete_orientation(&pending, Err(error.clone()));
            }
            if error.code == "orientation_unsupported"
                && let Some(state) = self.remote_orientation.as_mut()
            {
                state.support = OrientationSupport::Unsupported;
            }
            return Err(error);
        }
        pending.generation = self.semantic.generation();
        self.pending_orientation = Some(pending);
        Ok(())
    }
    pub(super) fn finish_orientation(&mut self, code: &str, message: &str) {
        if let Some(pending) = self.pending_orientation.take() {
            if let (Some(coordination), Origin::Agent(request)) = (&self.config.coordination, &pending.origin) {
                coordination.retain_action_result_on_remove(&self.config.panel_local_id, &request.action_id);
            }
            self.complete_orientation(&pending, Err(BrowserControlFailure::new(code, message)));
        }
    }
    pub(super) fn tick_orientation(&mut self, events: &BrowserEventSender, stopped: &AtomicBool) {
        let Some(mut pending) = self.pending_orientation.take() else {
            return;
        };
        if matches!(pending.origin, Origin::User { .. }) {
            self.stamp_user_active();
        }
        let result = self.advance_orientation(&mut pending, stopped, events);
        match result {
            Ok(Some(viewport)) => {
                // Publish a fresh frame and invalidate refs made during the transition.
                self.semantic.invalidate();

                if let Some(state) = self.remote_orientation.as_mut() {
                    state.support = OrientationSupport::Supported;
                    state.applied = Some(pending.requested);
                }
                self.coordination_dirty = true;
                self.write_coordination(true);
                self.complete_orientation(
                    &pending,
                    Ok(BrowserControlValue::Orientation {
                        requested: pending.requested,
                        applied: pending.requested,
                        viewport,
                    }),
                );
            }
            Ok(None) => self.pending_orientation = Some(pending),
            Err(error) => {
                if error.code == "orientation_unsupported"
                    && let Some(state) = self.remote_orientation.as_mut()
                {
                    state.support = OrientationSupport::Unsupported;
                    self.coordination_dirty = true;
                }
                self.complete_orientation(&pending, Err(error));
            }
        }
        if self.pending_orientation.is_none() {
            self.publish_orientation(events, None);
        }
    }
    fn guard_orientation(&mut self, pending: &Pending, stopped: &AtomicBool) -> Result<(), BrowserControlFailure> {
        if stopped.load(Ordering::Acquire) || self.host.has_exited() {
            return Err(BrowserControlFailure::new(
                "browser_unavailable",
                "browser stopped; orientation may have changed",
            ));
        }
        if !pending.origin.owns(self) {
            return Err(BrowserControlFailure::new(
                "orientation_ownership_lost",
                "ownership changed while rotating; inspect applied orientation",
            ));
        }
        if self.handoff_seen.is_some() && matches!(pending.origin, Origin::Agent(_)) {
            return Err(BrowserControlFailure::new(
                "orientation_handoff_pending",
                "handoff started while rotating; inspect applied orientation",
            ));
        }
        if pending.origin.human_took_over(self) {
            return Err(BrowserControlFailure::new(
                "orientation_user_active",
                "human input or Teach mode took over; inspect applied orientation",
            ));
        }
        remaining(pending.deadline)?;
        if pending.generation != self.semantic.generation()
            || self.classic_navigation_in_flight()
            || self.pending_navigation.is_some()
        {
            return Err(BrowserControlFailure::new(
                "orientation_navigation_invalidated",
                "the document changed while rotating; inspect applied orientation",
            ));
        }
        Ok(())
    }
    fn advance_orientation(
        &mut self,
        pending: &mut Pending,
        stopped: &AtomicBool,
        events: &BrowserEventSender,
    ) -> Result<Option<[u32; 2]>, BrowserControlFailure> {
        self.guard_orientation(pending, stopped)?;
        if let Some((viewport, epoch)) = pending.verified {
            if self.signal_epoch > epoch
                || (self.config.coordination.is_none() && matches!(pending.origin, Origin::User { .. }))
            {
                return Ok(Some(viewport));
            }
            return Ok(None);
        }
        if Instant::now() < pending.next_sample {
            return Ok(None);
        }
        let result = observe(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            pending.requested,
            pending.deadline,
        );
        pending.next_sample = Instant::now() + Duration::from_millis(100);
        match result {
            Ok(Some(measured)) => {
                self.frames.invalidate();
                let frame_slot = self.config.frame_slot.clone();
                let old_sequence = frame_slot.latest().map(|frame| frame.seq);
                self.capture_frame_until(&frame_slot, events, Some(pending.deadline));
                remaining(pending.deadline)?;
                if !frame_slot.latest().is_some_and(|frame| {
                    Some(frame.seq) != old_sequence
                        && frame.width != frame.height
                        && (frame.width > frame.height) == (pending.requested == RemoteOrientation::Landscape)
                }) {
                    return Ok(None);
                }
                self.refresh_classic_document_identity_within(
                    remaining(pending.deadline)?.min(Duration::from_secs(3)),
                )?;
                self.guard_orientation(pending, stopped)?;
                pending.verified = Some(([measured.width, measured.height], self.signal_epoch));
                // Only a subsequent successful coordination read can acknowledge
                // ownership after all blocking driver calls. Its actions stay
                // on the normal servicing path rather than being drained here.
                self.request_signal_refresh();
                Ok(None)
            }
            Err(error) if error.code == "orientation_unsupported" => Err(error),
            _ => {
                remaining(pending.deadline)?;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests;
