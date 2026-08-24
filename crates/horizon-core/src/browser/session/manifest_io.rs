//! Manifest transactions and agent/user handoff signals for the browser
//! driver. The parent session module owns lifecycle/CDP state; this leaf keeps
//! filesystem synchronization and TTL behavior out of that event loop.

use std::sync::mpsc;
use std::time::Instant;

use crate::browser::manifest::{self, BrowserManifest};

use super::{BrowserEvent, DriverState, MANIFEST_MIN_INTERVAL, SIGNAL_MIN_INTERVAL};

impl DriverState {
    /// User clicked "hand back": mark the pending handoff done so the
    /// blocked agent process can continue.
    pub(super) fn resolve_handoff(&mut self, event_tx: &mpsc::Sender<BrowserEvent>) {
        let mut acknowledged = false;
        let result = self.write_manifest_extra(true, |manifest| {
            acknowledged = acknowledge_handoff(manifest);
        });
        match result.and_then(|()| {
            acknowledged
                .then_some(())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "pending handoff no longer exists"))
        }) {
            Ok(()) => {
                self.handoff_seen = None;
                let _ = event_tx.send(BrowserEvent::HandoffCleared);
            }
            Err(error) => {
                tracing::warn!(target: "browser", "failed to resolve handoff: {error}");
                let _ = event_tx.send(BrowserEvent::HandoffResolutionFailed(error.to_string()));
            }
        }
    }

    /// Poll the manifest for agent-side signals (owner heartbeat, handoff
    /// requests). Cheap file read, throttled.
    pub(super) fn tick_signals(&mut self, tx: &mpsc::Sender<BrowserEvent>) {
        if self.last_signal_check.elapsed() < SIGNAL_MIN_INTERVAL {
            return;
        }
        self.last_signal_check = Instant::now();
        self.expire_user_active();
        let Some(manifest) = manifest::read(&self.config.panel_local_id) else {
            return;
        };
        let now = manifest::now_millis();
        let owner = manifest.live_owner(now).map(|owner| owner.name.clone());
        publish_owner_change(&mut self.owner_seen, owner, tx);
        match manifest.handoff_pending() {
            Some(handoff) => {
                if self.handoff_seen != Some(handoff.requested_at) {
                    self.handoff_seen = Some(handoff.requested_at);
                    let _ = tx.send(BrowserEvent::HandoffRequested(handoff.reason.clone()));
                }
            }
            None => {
                if self.handoff_seen.is_some() {
                    self.handoff_seen = None;
                    let _ = tx.send(BrowserEvent::HandoffCleared);
                }
            }
        }
    }

    /// Persist the shared manifest, preserving agent-owned fields. The
    /// driver's in-memory state is authoritative for its fields; the on-disk
    /// manifest is the base for `handoff` and `owner`.
    pub(super) fn write_manifest_extra(
        &mut self,
        force: bool,
        extra: impl FnOnce(&mut BrowserManifest),
    ) -> std::io::Result<()> {
        if !force && !self.manifest_dirty {
            return Ok(());
        }
        if !force && self.last_manifest_write.elapsed() < MANIFEST_MIN_INTERVAL {
            self.manifest_dirty = true;
            return Ok(());
        }
        let local_id = &self.config.panel_local_id;
        if !force && manifest::read(local_id).is_none() {
            self.manifest_dirty = true;
            return Ok(());
        }
        let update_result = manifest::update(local_id, |manifest| {
            extra(manifest);
            manifest.browser_ws.clone_from(&self.browser_ws);
            manifest
                .target_id
                .clone_from(&self.target_id.clone().unwrap_or_default());
            manifest.url.clone_from(&self.url);
            manifest.title.clone_from(&self.title);
            manifest.updated_at = manifest::now_millis();
        });
        match update_result {
            Ok(_) => {
                self.manifest_dirty = false;
                self.last_manifest_write = Instant::now();
                Ok(())
            }
            Err(error) => {
                self.manifest_dirty = true;
                Err(error)
            }
        }
    }

    pub(super) fn write_manifest(&mut self, force: bool) {
        if let Err(error) = self.write_manifest_extra(force, |_| {}) {
            tracing::warn!(target: "browser", "failed to write browser manifest: {error}");
        }
    }

    pub(super) fn initialize_manifest(&mut self) {
        if let Err(error) = self.write_manifest_extra(true, |manifest| {
            manifest.user_active = false;
            manifest.user_active_at = 0;
        }) {
            tracing::warn!(target: "browser", "failed to initialize browser manifest: {error}");
        }
    }

    fn expire_user_active(&mut self) {
        let Some(last_stamp) = self.last_user_active_stamp else {
            return;
        };
        if last_stamp.elapsed() < manifest::USER_ACTIVE_TTL {
            return;
        }
        match self.write_manifest_extra(true, |manifest| manifest.user_active = false) {
            Ok(()) => self.last_user_active_stamp = None,
            Err(error) => tracing::warn!(target: "browser", "failed to expire user activity: {error}"),
        }
    }

    pub(super) fn remove_manifest(&self) {
        manifest::remove(&self.config.panel_local_id);
    }
}

fn acknowledge_handoff(manifest: &mut BrowserManifest) -> bool {
    let Some(handoff) = manifest.handoff.as_mut() else {
        return false;
    };
    handoff.done = true;
    true
}

fn publish_owner_change(owner_seen: &mut Option<String>, owner: Option<String>, event_tx: &mpsc::Sender<BrowserEvent>) {
    if owner == *owner_seen {
        return;
    }
    owner_seen.clone_from(&owner);
    let _ = event_tx.send(BrowserEvent::OwnerChanged(owner));
}

#[cfg(test)]
mod tests {
    use crate::browser::manifest::ManifestHandoff;

    use super::*;

    #[test]
    fn acknowledgement_requires_a_manifest_handoff() {
        let mut manifest = BrowserManifest::default();
        assert!(!acknowledge_handoff(&mut manifest));

        manifest.handoff = Some(ManifestHandoff {
            reason: "captcha".to_string(),
            requested_at: 1,
            done: false,
        });
        assert!(acknowledge_handoff(&mut manifest));
        assert!(manifest.handoff.is_some_and(|handoff| handoff.done));
    }

    #[test]
    fn owner_change_keeps_and_publishes_the_live_owner() {
        let (tx, rx) = mpsc::channel();
        let mut owner_seen = None;

        publish_owner_change(&mut owner_seen, Some("agent-1".to_string()), &tx);

        assert_eq!(owner_seen.as_deref(), Some("agent-1"));
        assert_eq!(rx.recv(), Ok(BrowserEvent::OwnerChanged(Some("agent-1".to_string()))));
    }
}
