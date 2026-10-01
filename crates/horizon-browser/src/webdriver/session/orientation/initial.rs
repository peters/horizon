//! Publish startup orientation only against the committed document.
use super::super::Driver;
use crate::remote::OrientationSupport;
use crate::session::BrowserEventSender;

impl Driver {
    pub(in super::super) fn refresh_initial_orientation(
        &mut self,
        navigation_pending: bool,
        events: &BrowserEventSender,
    ) {
        let Some(state) = self.remote_orientation.as_mut() else {
            return;
        };
        self.initial_orientation_pending = navigation_pending;
        if navigation_pending {
            // The allocation probe measured the previous document.
            state.applied = None;
        } else if self.pending_orientation.is_some() {
            // A runtime request now owns the measurement and acknowledgement.
            return;
        } else if state.support != OrientationSupport::Unsupported {
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
