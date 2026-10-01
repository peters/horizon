//! User and agent origins share verification without fabricating agent ownership.
use super::{Driver, Pending};
use crate::remote::{OrientationSupport, RemoteOrientation, RemoteOrientationState, RemoteOrientationView};
use crate::session::{BrowserEvent, BrowserEventSender};
use crate::{
    AgentAction, BrowserAuditAction, BrowserAuditActor, BrowserAuditStatus, BrowserControlFailure, BrowserControlValue,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) enum Origin {
    Agent(AgentAction),
    User { action_id: String },
}
impl Origin {
    pub(super) fn owns(&self, driver: &Driver) -> bool {
        match self {
            Self::Agent(request) => driver.owner_seen.as_deref() == Some(request.actor.as_str()),
            Self::User { .. } => true,
        }
    }
    pub(super) fn human_took_over(&self, driver: &Driver) -> bool {
        match self {
            Self::Agent(_) => driver.orientation_human_active(),
            Self::User { .. } => driver.panel_slot.teach_recording(),
        }
    }
}
impl Driver {
    pub(in super::super) fn publish_orientation(&self, events: &BrowserEventSender, error: Option<String>) {
        if let Some(state) = self.remote_orientation {
            let _ = events.send(BrowserEvent::OrientationChanged(RemoteOrientationView {
                completed: self.orientation_completed.completed.clone(),
                action_id: self.orientation_action_id.clone(),
                state,
                pending: self.pending_orientation.as_ref().map(|pending| pending.requested),
                error: error.or_else(|| self.orientation_error.clone()),
            }));
        }
    }
    pub(in super::super) fn begin_user_orientation(
        &mut self,
        action_id: String,
        requested: RemoteOrientation,
        events: &BrowserEventSender,
        stopped: &AtomicBool,
    ) {
        self.stamp_user_active();
        let origin = Origin::User { action_id };
        self.orientation_action_id = match &origin {
            Origin::User { action_id } => Some(action_id.clone()),
            Origin::Agent(_) => None,
        };
        let pending = Pending {
            origin,
            requested,
            deadline: Instant::now() + Duration::from_millis(RemoteOrientation::DEFAULT_TIMEOUT_MILLIS),
            next_sample: Instant::now(),
            generation: self.semantic.generation(),
            verified: None,
        };
        let failure = if stopped.load(Ordering::Acquire) {
            Some(BrowserControlFailure::new(
                "browser_unavailable",
                "the browser is stopping",
            ))
        } else if self
            .remote_orientation
            .is_none_or(|state| state.support == OrientationSupport::Unsupported)
        {
            Some(BrowserControlFailure::new(
                "orientation_unsupported",
                "this endpoint does not support orientation",
            ))
        } else if self.panel_slot.teach_recording() {
            Some(BrowserControlFailure::new(
                "orientation_user_active",
                "stop Teach mode before rotating",
            ))
        } else {
            None
        };
        if let Some(error) = failure {
            self.audit_user_orientation(&pending, BrowserAuditStatus::Rejected);
            self.complete_orientation(&pending, Err(error));
        } else {
            self.audit_user_orientation(&pending, BrowserAuditStatus::Dispatched);
            self.orientation_error = None;
            if let Err(error) = self.dispatch_orientation(pending, events, stopped) {
                self.orientation_error = Some(format!("{}: {}", error.code, error.message));
            }
        }
        if self.remote_orientation.is_none() {
            let _ = events.send(BrowserEvent::OrientationChanged(RemoteOrientationView {
                completed: self.orientation_completed.completed.clone(),
                action_id: self.orientation_action_id.clone(),
                state: RemoteOrientationState {
                    support: OrientationSupport::Unsupported,
                    applied: None,
                },
                pending: None,
                error: self.orientation_error.clone(),
            }));
        } else {
            self.publish_orientation(events, None);
        }
    }
    fn audit_user_orientation(&self, pending: &Pending, status: BrowserAuditStatus) {
        if let Origin::User { action_id, .. } = &pending.origin {
            self.record_audit(
                action_id.clone(),
                BrowserAuditActor::User,
                status,
                BrowserAuditAction::Orientation {
                    orientation: pending.requested,
                },
            );
        }
    }
    pub(super) fn complete_orientation(
        &mut self,
        pending: &Pending,
        outcome: Result<BrowserControlValue, BrowserControlFailure>,
    ) {
        self.orientation_error = outcome
            .as_ref()
            .err()
            .map(|error| format!("{}: {}", error.code, error.message));
        if let Origin::User { action_id } = &pending.origin {
            self.orientation_completed
                .record_completion(crate::remote::RemoteOrientationCompletion {
                    action_id: action_id.clone(),
                    error: self.orientation_error.clone(),
                });
        }
        match &pending.origin {
            Origin::Agent(request) => self.complete_agent_action(request, outcome),
            Origin::User { .. } => self.audit_user_orientation(
                pending,
                if outcome.is_ok() {
                    BrowserAuditStatus::Completed
                } else {
                    BrowserAuditStatus::Failed
                },
            ),
        }
    }
}
