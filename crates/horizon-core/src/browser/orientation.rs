//! Host-side queuing of user rotation, shared by local and cloud panels.
use super::{BrowserCommand, BrowserPanelState, remote::RemoteOrientation};
impl BrowserPanelState {
    pub(super) fn apply_orientation_view(&mut self, mut view: super::remote::RemoteOrientationView) {
        if view.pending.is_some() && self.orientation_ack_expired() {
            return;
        }
        let completion = self
            .orientation
            .action_id
            .as_ref()
            .and_then(|id| view.completed.iter().find(|entry| &entry.action_id == id));
        if self.orientation.pending.is_none() || self.orientation.action_id == view.action_id || completion.is_some() {
            if let Some(completion) = completion {
                view.error.clone_from(&completion.error);
                view.action_id.clone_from(&self.orientation.action_id);
            }
            self.orientation = view;
            if self.orientation.pending.is_none() {
                self.orientation_pending_since = None;
            }
        }
    }
    fn orientation_ack_expired(&self) -> bool {
        self.orientation_pending_since.is_some_and(|since| {
            since.elapsed() > std::time::Duration::from_millis(RemoteOrientation::DEFAULT_TIMEOUT_MILLIS + 5_000)
        })
    }
    pub(super) fn expire_orientation_pending(&mut self) -> bool {
        if self.orientation.pending.is_some() && self.orientation_ack_expired() {
            self.orientation.pending = None;
            self.orientation.error = Some("orientation_status_unavailable: acknowledgement was lost; the device may rotate, inspect before retrying".into());
            return true;
        }
        false
    }
    pub fn request_orientation(&mut self, orientation: RemoteOrientation) {
        if !self.is_remote() {
            self.orientation.error = Some("orientation_unsupported: local browsers use viewport resize".into());
            return;
        }
        let action_id = horizon_browser_protocol::new_action_id();
        if self.try_send(BrowserCommand::Orientation {
            action_id: action_id.clone(),
            orientation,
        }) {
            self.orientation.action_id = Some(action_id);
            self.orientation.pending = Some(orientation);
            self.orientation_pending_since = Some(std::time::Instant::now());
            self.orientation.error = None;
        } else {
            self.orientation.error = Some("Rotation was not queued; the browser driver is unavailable or busy".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{
        BrowserDrainOutput, BrowserEvent, BrowserStatus,
        remote::{OrientationSupport, RemoteOrientationState, RemoteOrientationView},
    };
    #[test]
    fn runtime_failure_preserves_ready_panel_and_queue_rejection_is_not_pending() {
        let mut panel = BrowserPanelState::inert();
        panel.status = BrowserStatus::Ready;
        panel.request_orientation(RemoteOrientation::Landscape);
        assert!(panel.orientation.pending.is_none());
        assert!(panel.orientation.error.is_some());
        let mut output = BrowserDrainOutput::default();
        panel.apply_event(
            BrowserEvent::OrientationChanged(RemoteOrientationView {
                completed: Vec::new(),
                action_id: None,
                state: RemoteOrientationState {
                    support: OrientationSupport::Supported,
                    applied: None,
                },
                pending: None,
                error: Some("orientation_timeout: inspect before retrying".into()),
            }),
            &mut output,
        );
        assert!(matches!(panel.status, BrowserStatus::Ready));
        assert!(output.had_output);
        assert!(
            panel
                .orientation
                .error
                .as_deref()
                .unwrap()
                .contains("orientation_timeout")
        );
    }
    #[test]
    fn lost_or_evicted_acknowledgement_cannot_leave_controls_pending_forever() {
        let mut panel = BrowserPanelState::inert();
        panel.orientation.pending = Some(RemoteOrientation::Landscape);
        panel.orientation_pending_since = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30));
        assert!(panel.expire_orientation_pending());
        assert!(panel.orientation.pending.is_none());
        assert!(
            panel
                .orientation
                .error
                .as_deref()
                .unwrap()
                .contains("inspect before retrying")
        );
        panel.apply_orientation_view(RemoteOrientationView {
            pending: Some(RemoteOrientation::Landscape),
            ..RemoteOrientationView::default()
        });
        assert!(
            panel.orientation.pending.is_none(),
            "stale pending polls cannot resurrect a lost acknowledgement"
        );
    }
}
