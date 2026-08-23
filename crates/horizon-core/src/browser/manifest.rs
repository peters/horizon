//! File-based ownership/handoff manifest for browser panels.
//!
//! Each browser panel writes a small JSON file under
//! `~/.horizon/runtime/browsers/<panel_local_id>.json` while it is alive.
//! It is the shared channel between three parties:
//!
//! - the **panel driver** (writes `browser_ws`, `target_id`, `url`,
//!   `title`, `user_active`, and the handoff `done` flag),
//! - **external agents** (read discovery fields, heartbeat `owner`, write
//!   a `handoff` request),
//! - the **UI** (reads ownership for the chrome chip).
//!
//! File-based on purpose: no new IPC mechanism, works across process
//! boundaries, and every field is observable with plain shell tooling.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::horizon_home::HorizonHome;

/// How long an agent owner heartbeat stays fresh.
pub const OWNER_TTL_MILLIS: i64 = 10_000;

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
    /// Browser-level `DevTools` ws endpoint (what an agent connects to).
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
/// The temp file is pid-unique: the driver, an external agent, and the UI can
/// write the same manifest concurrently and must not clobber each other's
/// temp contents.
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
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&temp, raw.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }
    replace_file(&temp, path)?;
    Ok(())
}

/// Atomic replace that also works on Windows, where `std::fs::rename` does
/// not overwrite an existing destination (delete first, then retry once).
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    if std::fs::rename(temp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(temp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(temp);
        })
    } else {
        Ok(())
    }
}

pub fn remove(panel_local_id: &str) {
    let _ = std::fs::remove_file(default_manifest_path(panel_local_id));
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
