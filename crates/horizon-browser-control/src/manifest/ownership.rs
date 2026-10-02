//! Read-only ownership observations never claim queued controls or normalize handoffs.
use std::path::Path;

use horizon_browser::{CoordinationOwnership, HandoffRequest};

use super::{default_manifest_path, host_instance, now_millis, try_read_at};

pub(super) fn observe(panel_local_id: &str) -> std::io::Result<CoordinationOwnership> {
    observe_at(&default_manifest_path(panel_local_id), host_instance(), now_millis)
}

fn observe_at(path: &Path, host: &str, clock: impl FnOnce() -> i64) -> std::io::Result<CoordinationOwnership> {
    let snapshot = try_read_at(path)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "browser ownership manifest is unavailable",
        )
    })?;
    if snapshot.host.as_deref() != Some(host) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser manifest was adopted by another Horizon host",
        ));
    }
    let now = clock();
    Ok(CoordinationOwnership {
        owner: snapshot.live_owner(now).map(|owner| owner.name.clone()),
        handoff: snapshot.handoff_pending().map(|handoff| HandoffRequest {
            request_id: handoff.request_id.clone(),
            reason: handoff.reason.clone(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BrowserManifest, ManifestHandoff, ManifestOwner, OWNER_TTL_MILLIS, write_at};
    use horizon_browser::{AgentAction, BrowserControlAction};

    #[test]
    fn observations_preserve_actions_legacy_handoff_and_manifest_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let manifest = BrowserManifest {
            host: Some("test-host".into()),
            owner: Some(ManifestOwner {
                name: "agent".into(),
                tty: None,
                updated_at: 1000,
            }),
            handoff: Some(ManifestHandoff {
                request_id: String::new(),
                reason: "pending legacy handoff".into(),
                requested_at: 1000,
                done: false,
            }),
            actions: vec![AgentAction {
                action_id: "queued-action".into(),
                actor: "agent".into(),
                requested_at_millis: 1000,
                action: BrowserControlAction::Reload,
            }],
            ..Default::default()
        };
        write_at(&path, &manifest).unwrap();
        let before = std::fs::read(&path).unwrap();
        let observed = observe_at(&path, "test-host", || 1000).unwrap();
        assert_eq!(observed.owner.as_deref(), Some("agent"));
        assert_eq!(observed.handoff.unwrap().request_id, "");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(try_read_at(&path).unwrap().unwrap().actions, manifest.actions);
        assert!(
            observe_at(&path, "test-host", || 1001 + OWNER_TTL_MILLIS)
                .unwrap()
                .owner
                .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn heartbeat_after_pre_read_clock_is_evaluated_at_snapshot_time() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let pre_read_clock = 1000;
        let mut manifest = BrowserManifest {
            host: Some("test-host".into()),
            owner: Some(ManifestOwner {
                name: "agent".into(),
                tty: None,
                updated_at: pre_read_clock + 1,
            }),
            ..Default::default()
        };
        write_at(&path, &manifest).unwrap();
        let observed = observe_at(&path, "test-host", || {
            manifest.owner.as_mut().unwrap().name = "later-owner".into();
            write_at(&path, &manifest).unwrap();
            pre_read_clock + 1
        })
        .unwrap();
        assert_eq!(
            observed.owner.as_deref(),
            Some("agent"),
            "read the snapshot before sampling its clock"
        );
        assert_eq!(try_read_at(&path).unwrap().unwrap().owner.unwrap().name, "later-owner");
    }

    #[test]
    fn missing_corrupt_and_adopted_manifests_fail_closed_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        assert_eq!(
            observe_at(&path, "test-host", || 1000).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(!path.exists());
        std::fs::write(&path, "corrupt manifest").unwrap();
        assert!(observe_at(&path, "test-host", || 1000).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt manifest");
        let manifest = BrowserManifest {
            host: Some("new-host".into()),
            ..Default::default()
        };
        write_at(&path, &manifest).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            observe_at(&path, "test-host", || 1000).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
