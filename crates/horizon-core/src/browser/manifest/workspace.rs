//! Host-authoritative workspace membership for browser MCP authorization.
//!
//! Browser manifests are shared by every Horizon process using the same
//! Horizon home, so a panel id alone never proves that the calling agent may
//! control a panel. Only the Horizon host knows board membership, so it stamps
//! each live browser manifest with the agent identities currently sharing the
//! panel's workspace. The MCP adapter authorizes a workspace-scoped caller by
//! that membership on every call: moving either panel between workspaces
//! changes the stamp on the host's next sync without restarting the browser
//! session or the agent. Agent identities are per-panel UUIDs, so two hosts
//! sharing one home never stamp each other's identities even when their
//! workspace ids collide. A manifest without a stamp fails closed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{default_manifest_path, now_millis, read_at, update_at};

/// The workspace a browser panel currently belongs to, as its host sees it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestWorkspace {
    /// Persisted local id of the workspace, for diagnostics only.
    pub local_id: String,
    /// Sorted, de-duplicated identities of the agent panels in that
    /// workspace: the only workspace-scoped callers allowed to discover or
    /// control the panel.
    #[serde(default)]
    pub actors: Vec<String>,
}

impl ManifestWorkspace {
    #[must_use]
    pub fn new(local_id: &str, mut actors: Vec<String>) -> Self {
        actors.sort();
        actors.dedup();
        Self {
            local_id: local_id.to_string(),
            actors,
        }
    }

    #[must_use]
    pub fn authorizes(&self, actor: &str) -> bool {
        self.actors.iter().any(|candidate| candidate == actor)
    }
}

/// Stamp host-owned presentation and placement state on a live manifest,
/// writing only when something differs. Returns whether a write happened.
///
/// # Errors
/// Returns `NotFound` when the panel is not live, or another error when the
/// locked manifest transaction fails.
pub fn sync_host_state(panel_local_id: &str, visible: bool, workspace: &ManifestWorkspace) -> std::io::Result<bool> {
    sync_host_state_at(
        &default_manifest_path(panel_local_id),
        panel_local_id,
        visible,
        workspace,
    )
}

fn sync_host_state_at(
    path: &Path,
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<bool> {
    let hidden = !visible;
    let current = read_at(path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("browser manifest is not live: {}", path.display()),
        )
    })?;
    if current.hidden == hidden && current.workspace.as_ref() == Some(workspace) {
        return Ok(false);
    }
    update_at(path, panel_local_id, |manifest| {
        manifest.hidden = hidden;
        manifest.workspace = Some(workspace.clone());
        manifest.updated_at = now_millis();
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::{BrowserManifest, manifest_path_for_root, write_at};

    #[test]
    fn membership_is_exact_and_an_unstamped_manifest_authorizes_nobody() {
        let workspace = ManifestWorkspace::new(
            "ws-a",
            vec![
                "horizon:agent-b".to_string(),
                "horizon:agent-a".to_string(),
                "horizon:agent-a".to_string(),
            ],
        );
        assert_eq!(workspace.actors, ["horizon:agent-a", "horizon:agent-b"]);
        assert!(workspace.authorizes("horizon:agent-a"));
        assert!(!workspace.authorizes("horizon:agent-c"));
        assert!(!workspace.authorizes("horizon:agent"));

        let mut manifest = BrowserManifest::default();
        assert!(!manifest.authorizes_actor("horizon:agent-a"));
        manifest.workspace = Some(workspace);
        assert!(manifest.authorizes_actor("horizon:agent-a"));
        assert!(!manifest.authorizes_actor("horizon:agent-c"));
    }

    #[test]
    fn host_state_sync_writes_only_when_presentation_or_membership_changes() {
        let root = tempfile::tempdir().expect("isolated root");
        let path = manifest_path_for_root(root.path(), "panel");
        write_at(
            &path,
            &BrowserManifest {
                panel_local_id: "panel".to_string(),
                ..BrowserManifest::default()
            },
        )
        .expect("write manifest");
        let workspace = ManifestWorkspace::new("ws-a", vec!["horizon:agent-a".to_string()]);

        assert!(sync_host_state_at(&path, "panel", true, &workspace).expect("stamp"));
        let stamped = read_at(&path).expect("read stamped");
        assert!(!stamped.hidden);
        assert_eq!(stamped.workspace.as_ref(), Some(&workspace));
        assert!(!sync_host_state_at(&path, "panel", true, &workspace).expect("steady state"));

        assert!(sync_host_state_at(&path, "panel", false, &workspace).expect("hide"));
        assert!(read_at(&path).expect("read hidden").hidden);

        let moved = ManifestWorkspace::new("ws-b", vec!["horizon:agent-b".to_string()]);
        assert!(sync_host_state_at(&path, "panel", false, &moved).expect("move"));
        let after_move = read_at(&path).expect("read moved");
        assert!(after_move.authorizes_actor("horizon:agent-b"));
        assert!(!after_move.authorizes_actor("horizon:agent-a"));

        let missing = manifest_path_for_root(root.path(), "missing");
        assert_eq!(
            sync_host_state_at(&missing, "missing", true, &workspace)
                .expect_err("missing manifest must fail")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn legacy_manifests_deserialize_without_a_workspace_stamp() {
        let manifest: BrowserManifest = serde_json::from_str(
            r#"{
                "panel_local_id": "legacy",
                "browser_ws": "",
                "target_id": "",
                "url": "",
                "title": "",
                "user_active": false,
                "user_active_at": 0,
                "updated_at": 0
            }"#,
        )
        .expect("legacy manifest parses");

        assert!(manifest.workspace.is_none());
        assert!(!manifest.authorizes_actor("horizon:agent-a"));
    }
}
