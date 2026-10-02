//! Explicit starts require verified geometry on the first committed page.
use super::super::Driver;
use crate::remote::{OrientationSupport, RemoteOrientation, RemoteOrientationState};
use crate::session::{BrowserEvent, BrowserEventSender};
use crate::{RemoteReleaseOutcome, RemoteSessionEvent};

fn rejection(
    requested: RemoteOrientation,
    state: Option<RemoteOrientationState>,
    navigation_unverified: bool,
) -> Option<&'static str> {
    if navigation_unverified {
        return Some("orientation_unverified");
    }
    match state {
        Some(state) if state.support == OrientationSupport::Unsupported => Some("orientation_unsupported"),
        Some(state) if state.applied == Some(requested) => None,
        Some(state) if state.applied.is_some() => Some("remote_orientation_mismatch"),
        _ => Some("orientation_unverified"),
    }
}

impl Driver {
    pub(in super::super) fn reject_explicit_start_orientation(
        &mut self,
        navigation_pending: bool,
        stopped: bool,
        events: &BrowserEventSender,
    ) -> bool {
        let Some(request) = self.config.remote.as_ref() else {
            return false;
        };
        let Some(requested) = request.orientation() else {
            return false;
        };
        let code = if stopped {
            Some("browser_unavailable")
        } else {
            rejection(
                requested,
                self.remote_orientation,
                navigation_pending || self.navigation_failed,
            )
        };
        let Some(code) = code else { return false };
        let label = request.label.clone();
        let released = self
            .host
            .release(&self.session_id)
            .unwrap_or(RemoteReleaseOutcome::NeverAllocated);
        *self
            .remote_release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(released.clone());
        let _ = events.send(BrowserEvent::RemoteSession(RemoteSessionEvent::OrientationRejected {
            label,
            code,
            released,
        }));
        true
    }
}
