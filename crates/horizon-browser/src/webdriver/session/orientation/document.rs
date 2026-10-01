//! Observe remote orientation against the current committed document.
use super::super::Driver;
use crate::remote::OrientationSupport;
use crate::session::BrowserEventSender;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum DocumentOrientation {
    Clean,
    NeedsPublication,
    NeedsMeasurement,
}

impl DocumentOrientation {
    pub(in super::super) fn initial(state: Option<crate::remote::RemoteOrientationState>) -> Self {
        state.map_or(Self::Clean, |_| Self::NeedsPublication)
    }
}

impl Driver {
    pub(in super::super) fn invalidate_document_orientation(&mut self) {
        if let Some(state) = self.remote_orientation.as_mut() {
            state.applied = None;
            self.orientation_document = DocumentOrientation::NeedsPublication;
            self.coordination_dirty = true;
        }
    }

    pub(in super::super) fn publish_document_orientation_invalidation(&mut self, events: &BrowserEventSender) {
        if self.orientation_document == DocumentOrientation::NeedsPublication {
            if let Some(state) = self.remote_orientation.as_mut() {
                state.applied = None;
            }
            self.publish_orientation(events, None);
            self.write_coordination(true);
            self.orientation_document = DocumentOrientation::NeedsMeasurement;
        }
    }

    pub(in super::super) fn refresh_document_orientation(&mut self, events: &BrowserEventSender) {
        self.publish_document_orientation_invalidation(events);
        if self.orientation_document != DocumentOrientation::NeedsMeasurement
            || self.classic_navigation_in_flight()
            || self.pending_orientation.is_some()
            || self.pending_wait.is_some()
            || self.pending_navigation.is_some()
        {
            return;
        }
        self.orientation_document = DocumentOrientation::Clean;
        if self
            .remote_orientation
            .is_some_and(|state| state.support != OrientationSupport::Unsupported)
        {
            self.remote_orientation = Some(super::super::super::orientation::probe(
                self.host.transport(),
                &format!("/session/{}", self.session_id),
            ));
        }
        self.coordination_dirty = true;
        self.write_coordination(true);
        self.publish_orientation(events, None);
    }
}
