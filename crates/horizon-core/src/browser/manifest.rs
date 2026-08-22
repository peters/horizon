//! File-based ownership/handoff manifest for browser panels.
//!
//! Each browser panel writes a small JSON file under
//! `~/.horizon/runtime/browsers/<panel_local_id>.json` while it is alive.
//! It is the shared channel between three parties:
//!
//! - the **panel driver** (writes `browser_ws`, `target_id`, `url`, `title`),
//! - the **agent CLI `hb`** (heartbeats `owner`, requests handoffs),
//! - the **UI** (reads ownership for the titlebar chip, writes
//!   `user_active` and the handoff `done` flag).
//!
//! File-based on purpose: no new IPC mechanism, works across process
//! boundaries, and every field is observable with plain shell tooling.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::horizon_home::HorizonHome;

/// How long an agent owner heartbeat stays fresh.
pub const OWNER_TTL_MILLIS: i64 = 10_000;
/// How long a user-active stamp stays fresh.
pub const USER_ACTIVE_TTL_MILLIS: i64 = 5_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestOwner {
    pub name: String,
    /// PTY the agent runs on (e.g. `pts/14`), when discoverable. Lets the UI
    /// map the owner back to the terminal panel that spawned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestHandoff {
    pub reason: String,
    pub requested_at: i64,
    /// Set by the UI when the user clicks "hand back".
    pub done: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct BrowserManifest {
    pub panel_local_id: String,
    /// Browser-level `DevTools` ws endpoint (what `hb` connects to).
    pub browser_ws: String,
    /// The page target the panel is driving.
    pub target_id: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ManifestOwner>,
    pub user_active: bool,
    pub user_active_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<ManifestHandoff>,
    pub updated_at: i64,
}

impl BrowserManifest {
    /// The owner, if it has heartbeat within [`OWNER_TTL_MILLIS`].
    #[must_use]
    pub fn live_owner(&self, now_millis: i64) -> Option<&ManifestOwner> {
        self.owner
            .as_ref()
            .filter(|owner| now_millis.saturating_sub(owner.updated_at) <= OWNER_TTL_MILLIS)
    }

    #[must_use]
    pub fn user_is_active(&self, now_millis: i64) -> bool {
        self.user_active && now_millis.saturating_sub(self.user_active_at) <= USER_ACTIVE_TTL_MILLIS
    }

    #[must_use]
    pub fn handoff_pending(&self) -> Option<&ManifestHandoff> {
        self.handoff.as_ref().filter(|h| !h.done)
    }
}

#[must_use]
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[must_use]
pub fn manifest_path_for_root(root: &Path, panel_local_id: &str) -> PathBuf {
    let base = root.join("runtime").join("browsers");
    // Sanitize: local ids are ours, but a hostile file name must not escape
    // the manifest directory.
    let safe: String = panel_local_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    base.join(format!("{safe}.json"))
}

#[must_use]
pub fn default_manifest_path(panel_local_id: &str) -> PathBuf {
    let home = HorizonHome::resolve();
    manifest_path_for_root(home.root(), panel_local_id)
}

#[must_use]
pub fn default_manifest_dir() -> PathBuf {
    HorizonHome::resolve().browsers_manifest_dir()
}

/// Read the manifest for a panel, if present and parseable.
#[must_use]
pub fn read(panel_local_id: &str) -> Option<BrowserManifest> {
    read_at(&default_manifest_path(panel_local_id))
}

#[must_use]
pub fn read_at(path: &Path) -> Option<BrowserManifest> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// List panel local ids that currently have a manifest.
#[must_use]
pub fn list_panels() -> Vec<String> {
    list_panels_in(&default_manifest_dir())
}

#[must_use]
pub fn list_panels_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let id = file_name.strip_suffix(".json")?;
            Some(id.to_string())
        })
        .collect()
}

/// Write the manifest atomically (temp file + rename), mode 0600.
///
/// # Errors
/// Fails on filesystem errors.
pub fn write(manifest: &BrowserManifest) -> std::io::Result<()> {
    write_at(&default_manifest_path(&manifest.panel_local_id), manifest)
}

/// Write to an explicit path (used by tests).
///
/// # Errors
/// Fails on filesystem errors.
pub fn write_at(path: &Path, manifest: &BrowserManifest) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(manifest).map_err(|e| std::io::Error::other(e.to_string()))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, raw.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub fn remove(panel_local_id: &str) {
    let _ = std::fs::remove_file(default_manifest_path(panel_local_id));
}

/// Refresh the agent owner heartbeat (called by `hb` on every command).
#[must_use]
pub fn owner_heartbeat(panel_local_id: &str, name: &str, tty: Option<String>) -> Option<()> {
    let path = default_manifest_path(panel_local_id);
    let mut manifest = read_at(&path)?;
    let now = now_millis();
    manifest.owner = Some(ManifestOwner {
        name: name.to_string(),
        tty,
        updated_at: now,
    });
    manifest.updated_at = now;
    write_at(&path, &manifest).ok()
}

/// Ask for a human handoff. Returns the manifest path so the caller can
/// poll for `done`.
///
/// The panel driver also rewrites this file (`url`/`title`/`browser_ws`), so a
/// read-modify-write race can briefly drop a just-written field; the
/// expected mitigation is on the caller side: re-assert the handoff
/// (call this again) while it is pending and not yet `done`.
#[must_use]
pub fn request_handoff(panel_local_id: &str, reason: &str) -> Option<PathBuf> {
    let path = default_manifest_path(panel_local_id);
    let mut manifest = read_at(&path)?;
    let now = now_millis();
    manifest.handoff = Some(ManifestHandoff {
        reason: reason.to_string(),
        requested_at: now,
        done: false,
    });
    manifest.updated_at = now;
    write_at(&path, &manifest).ok()?;
    Some(path)
}

/// Stamp the user as actively driving the panel.
pub fn set_user_active(panel_local_id: &str) {
    let path = default_manifest_path(panel_local_id);
    let Some(mut manifest) = read_at(&path) else {
        return;
    };
    let now = now_millis();
    manifest.user_active = true;
    manifest.user_active_at = now;
    manifest.updated_at = now;
    let _ = write_at(&path, &manifest);
}

/// Mark the pending handoff as complete (UI "hand back" button).
pub fn mark_handoff_done(panel_local_id: &str) {
    let path = default_manifest_path(panel_local_id);
    let Some(mut manifest) = read_at(&path) else {
        return;
    };
    if let Some(handoff) = manifest.handoff.as_mut() {
        handoff.done = true;
    }
    manifest.updated_at = now_millis();
    let _ = write_at(&path, &manifest);
}

/// Poll until the handoff is marked done or the timeout elapses.
#[must_use]
pub fn wait_handoff_done(panel_local_id: &str, timeout: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        if read(panel_local_id).and_then(|m| m.handoff).is_some_and(|h| h.done) {
            return true;
        }
        if started.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_root() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("horizon-browser-mani{}", std::process::id()))
            .join(format!("t{n}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn sample(id: &str) -> BrowserManifest {
        BrowserManifest {
            panel_local_id: id.to_string(),
            browser_ws: "ws://127.0.0.1:1/devtools/browser/x".to_string(),
            target_id: "T1".to_string(),
            url: "https://example.com".to_string(),
            title: "Example".to_string(),
            owner: None,
            user_active: false,
            user_active_at: 0,
            handoff: None,
            updated_at: 0,
        }
    }

    #[test]
    fn write_read_roundtrip() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "abc-123");
        let m = sample("abc-123");
        write_at(&path, &m).unwrap();
        let back = read_at(&path).unwrap();
        assert_eq!(back, m);
        assert!(list_panels_in(&root.join("runtime").join("browsers")).contains(&"abc-123".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_is_sanitized() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "../evil");
        assert!(path.starts_with(&root));
        assert!(!path.to_string_lossy().contains(".."));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn owner_ttl_expires() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "p1");
        let mut m = sample("p1");
        let now = 1_000_000i64;
        m.owner = Some(ManifestOwner {
            name: "pi".to_string(),
            tty: Some("pts/1".to_string()),
            updated_at: now - OWNER_TTL_MILLIS / 2,
        });
        write_at(&path, &m).unwrap();
        let read_back = read_at(&path).unwrap();
        assert!(read_back.live_owner(now).is_some());
        assert!(read_back.live_owner(now + OWNER_TTL_MILLIS).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handoff_lifecycle() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "p2");
        write_at(&path, &sample("p2")).unwrap();
        let mut m = read_at(&path).unwrap();
        m.handoff = Some(ManifestHandoff {
            reason: "captcha".to_string(),
            requested_at: now_millis(),
            done: false,
        });
        write_at(&path, &m).unwrap();
        assert!(read_at(&path).unwrap().handoff_pending().is_some());
        let mut m = read_at(&path).unwrap();
        m.handoff.as_mut().unwrap().done = true;
        write_at(&path, &m).unwrap();
        assert!(read_at(&path).unwrap().handoff_pending().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
