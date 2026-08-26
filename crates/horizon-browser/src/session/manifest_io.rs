//! Optional host coordination and agent/user handoff signals. Horizon owns
//! the filesystem manifest; the engine only calls this narrow boundary.

use std::time::Instant;

use crate::{CoordinationState, HandoffRequest};

use super::{
    BrowserEvent, BrowserEventSender, DriverState, MANIFEST_MIN_INTERVAL, SIGNAL_MIN_INTERVAL, USER_ACTIVE_TTL,
};

impl DriverState {
    /// User clicked "hand back": acknowledge only the exact request that the
    /// engine previously surfaced, so a replacement request cannot be lost.
    pub(super) fn resolve_handoff(&mut self, event_tx: &BrowserEventSender) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                "agent handoff coordination is unavailable".to_string(),
            ));
            return;
        };
        let Some(request_id) = self.handoff_seen.as_deref() else {
            let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                "pending handoff no longer exists".to_string(),
            ));
            return;
        };
        match coordination.acknowledge_handoff(&self.config.panel_local_id, request_id) {
            Ok(true) => {
                self.handoff_seen = None;
                let _ = event_tx.send(BrowserEvent::HandoffCleared);
            }
            Ok(false) => {
                let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(
                    "pending handoff no longer exists".to_string(),
                ));
            }
            Err(error) => {
                tracing::warn!(target: "browser", "failed to resolve handoff: {error}");
                let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(error.to_string()));
            }
        }
    }

    /// Poll product-owned coordination for owner and handoff changes.
    pub(super) fn tick_signals(&mut self, event_tx: &BrowserEventSender) {
        if self.last_signal_check.elapsed() < SIGNAL_MIN_INTERVAL {
            return;
        }
        self.last_signal_check = Instant::now();
        self.expire_user_active();
        let Some(coordination) = self.config.coordination.as_ref() else {
            return;
        };
        let signals = match coordination.signals(&self.config.panel_local_id) {
            Ok(signals) => signals,
            Err(error) => {
                tracing::debug!(target: "browser", "failed to read browser coordination: {error}");
                return;
            }
        };
        publish_owner_change(&mut self.owner_seen, signals.owner, event_tx);
        publish_handoff_change(&mut self.handoff_seen, signals.handoff, event_tx);
    }

    pub(super) fn write_manifest(&mut self, force: bool) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.manifest_dirty = false;
            return;
        };
        if !force && !self.manifest_dirty {
            return;
        }
        if !force && self.last_manifest_write.elapsed() < MANIFEST_MIN_INTERVAL {
            return;
        }
        match coordination.update(&self.config.panel_local_id, &self.coordination_state()) {
            Ok(()) => {
                self.manifest_dirty = false;
                self.last_manifest_write = Instant::now();
            }
            Err(error) => {
                self.manifest_dirty = true;
                tracing::warn!(target: "browser", "failed to update browser coordination: {error}");
            }
        }
    }

    pub(super) fn initialize_manifest(&mut self) {
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.manifest_dirty = false;
            return;
        };
        match coordination.initialize(&self.config.panel_local_id, &self.coordination_state()) {
            Ok(()) => {
                self.manifest_dirty = false;
                self.last_manifest_write = Instant::now();
            }
            Err(error) => {
                self.manifest_dirty = true;
                tracing::warn!(target: "browser", "failed to initialize browser coordination: {error}");
            }
        }
    }

    pub(super) fn stamp_user_active(&mut self) {
        if self
            .last_user_active_stamp
            .is_some_and(|last_stamp| last_stamp.elapsed() < super::USER_ACTIVE_STAMP_INTERVAL)
        {
            return;
        }
        let Some(coordination) = self.config.coordination.as_ref() else {
            return;
        };
        let stamped_at = Instant::now();
        match coordination.set_user_active(&self.config.panel_local_id, true) {
            Ok(()) => self.last_user_active_stamp = Some(stamped_at),
            Err(error) => tracing::warn!(target: "browser", "failed to stamp user activity: {error}"),
        }
    }

    fn expire_user_active(&mut self) {
        let Some(last_stamp) = self.last_user_active_stamp else {
            return;
        };
        if last_stamp.elapsed() < USER_ACTIVE_TTL {
            return;
        }
        let Some(coordination) = self.config.coordination.as_ref() else {
            self.last_user_active_stamp = None;
            return;
        };
        match coordination.set_user_active(&self.config.panel_local_id, false) {
            Ok(()) => self.last_user_active_stamp = None,
            Err(error) => tracing::warn!(target: "browser", "failed to expire user activity: {error}"),
        }
    }

    fn coordination_state(&self) -> CoordinationState {
        CoordinationState {
            browser_ws: self.browser_ws.clone(),
            target_id: self.target_id.clone().unwrap_or_default(),
            url: self.url.clone(),
            title: self.title.clone(),
        }
    }
}

fn publish_owner_change(owner_seen: &mut Option<String>, owner: Option<String>, event_tx: &BrowserEventSender) {
    if owner == *owner_seen {
        return;
    }
    owner_seen.clone_from(&owner);
    let _ = event_tx.send(BrowserEvent::OwnerChanged(owner));
}

fn publish_handoff_change(
    handoff_seen: &mut Option<String>,
    handoff: Option<HandoffRequest>,
    event_tx: &BrowserEventSender,
) {
    match handoff {
        Some(handoff) if handoff_seen.as_deref() != Some(handoff.request_id.as_str()) => {
            handoff_seen.clone_from(&Some(handoff.request_id));
            let _ = event_tx.send(BrowserEvent::HandoffRequested(handoff.reason));
        }
        None if handoff_seen.take().is_some() => {
            let _ = event_tx.send(BrowserEvent::HandoffCleared);
        }
        Some(_) | None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::publish_handoff_change;
    use crate::HandoffRequest;
    use crate::session::{BrowserEvent, BrowserEventSender, BrowserEventWake, CommittedUrl};
    use std::sync::mpsc;

    #[test]
    fn replacement_handoff_uses_its_own_identity() {
        let (tx, rx) = mpsc::channel();
        let sender = BrowserEventSender {
            tx,
            wake: BrowserEventWake::default(),
            committed_url: CommittedUrl::default(),
        };
        let mut seen = Some("old".to_string());
        publish_handoff_change(
            &mut seen,
            Some(HandoffRequest {
                request_id: "replacement".to_string(),
                reason: "new reason".to_string(),
            }),
            &sender,
        );
        assert_eq!(seen.as_deref(), Some("replacement"));
        assert_eq!(rx.recv(), Ok(BrowserEvent::HandoffRequested("new reason".to_string())));
    }
}
