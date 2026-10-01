//! Host-side queuing of user rotation, shared by local and cloud panels.
use super::{BrowserCommand, BrowserPanelState, remote::RemoteOrientation};
impl BrowserPanelState {
    pub(super) fn apply_orientation_view(&mut self, mut view: super::remote::RemoteOrientationView) {
        let completion = self
            .orientation
            .action_id
            .as_ref()
            .and_then(|id| view.completed.iter().find(|entry| &entry.action_id == id));
        let matching = self
            .orientation
            .action_id
            .as_ref()
            .is_some_and(|id| view.action_id.as_ref() == Some(id));
        let terminal = completion.is_some() || (matching && view.pending.is_none());
        // Expiry clears presentation pending, but keeps the unresolved local request.
        if self.orientation_ack_expired() && !terminal {
            return;
        }
        if self.orientation_pending_since.is_none() || matching || completion.is_some() {
            if let Some(completion) = completion {
                view.error.clone_from(&completion.error);
                view.action_id.clone_from(&self.orientation.action_id);
            }
            self.orientation = view;
            if terminal || self.orientation.pending.is_none() {
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
        if self.remote_target().is_none() {
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
mod tests;
