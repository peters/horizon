//! File-based ownership/handoff manifest for browser panels.
//!
//! Each browser panel writes a small JSON file under
//! `~/.horizon/runtime/browsers/<encoded-panel-local-id>.json` while it is
//! alive.
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
//! Every update to an existing manifest must use [`update`] (or implement its
//! adjacent `<panel>.json.lock` protocol around both the read and the write).
//! This serializes read-modify-write transactions across the driver and agent
//! processes so ownership/handoff fields cannot be lost. Driver startup uses
//! [`initialize`] to create the manifest explicitly; later updates fail when
//! teardown has removed it, so a delayed writer cannot resurrect a dead panel.

use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::horizon_home::{HorizonHome, safe_local_id};

/// How long an agent owner heartbeat stays fresh.
pub const OWNER_TTL_MILLIS: i64 = 10_000;
/// How long the driver's `user_active` signal stays true without another
/// qualifying interaction.
pub const USER_ACTIVE_TTL: Duration = Duration::from_secs(5);
const LOCK_WAIT: Duration = Duration::from_secs(2);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);
/// Teardown must not leave a live endpoint behind for the normal 30-second
/// stale-lock window. Manifest transactions are local, tiny file writes; a
/// lock held for this long cannot still represent a healthy transaction.
const REMOVE_STALE_LOCK_AGE: Duration = Duration::from_secs(1);
const LOCK_RETRY: Duration = Duration::from_millis(5);
static HANDOFF_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// Collision-resistant identity generated for each request (for example,
    /// a UUID). A replacement request must always use a new value, even when
    /// its reason and millisecond timestamp match the previous request.
    #[serde(default)]
    pub request_id: String,
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

    /// Whether the user-activity signal is both asserted and fresh.
    #[must_use]
    pub fn user_is_active(&self, now_millis: i64) -> bool {
        let ttl_millis = i64::try_from(USER_ACTIVE_TTL.as_millis()).unwrap_or(i64::MAX);
        self.user_active && now_millis.saturating_sub(self.user_active_at) <= ttl_millis
    }
}

#[must_use]
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Generate an opaque identity for one handoff request. The nanosecond clock,
/// process id, and per-process counter keep identities distinct across rapid
/// replacements and concurrent agent processes without another dependency.
#[must_use]
pub fn new_handoff_request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = HANDOFF_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{nanos:x}-{sequence:x}", std::process::id())
}

#[must_use]
pub fn manifest_path_for_root(root: &Path, panel_local_id: &str) -> PathBuf {
    let base = root.join("runtime").join("browsers");
    // Local ids are normally generated by Horizon, but persisted state is
    // untrusted and must never escape the runtime directory.
    let safe = safe_local_id(panel_local_id);
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

/// List panel local ids that currently have a valid, canonically named
/// manifest.
#[must_use]
pub fn list_panels_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let encoded_id = file_name.strip_suffix(".json")?;
            let manifest = read_at(&entry.path())?;
            (safe_local_id(&manifest.panel_local_id) == encoded_id).then_some(manifest.panel_local_id)
        })
        .collect()
}

/// Write a complete manifest atomically (temp file + rename), mode 0600.
///
/// The adjacent inter-process lock serializes the replacement itself, and the
/// pid-unique temp file keeps a crashed write from corrupting the live
/// manifest. Callers deriving the new value from an existing manifest must
/// use [`update`] so the read is covered by the same lock.
///
/// # Errors
/// Fails on filesystem errors.
pub fn write(manifest: &BrowserManifest) -> std::io::Result<()> {
    write_at(&default_manifest_path(&manifest.panel_local_id), manifest)
}

/// Write to an explicit path (used by tests).
///
/// The manifest carries a live CDP endpoint, so the temp file is created
/// `0600` from the first byte — never world-readable under a common umask.
///
/// # Errors
/// Fails on filesystem errors (including the permission tightening, which
/// also covers a stale temp file left behind by a crashed run — `mode()`
/// only applies at creation).
pub fn write_at(path: &Path, manifest: &BrowserManifest) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire(path)?;
    write_at_locked(path, manifest)
}

/// Initialize a driver's manifest, preserving any agent-owned fields already
/// written under the same lock.
///
/// Unlike [`update`], this operation may create a missing manifest. It is
/// reserved for the single driver-start boundary.
///
/// # Errors
/// Fails when the lock or manifest write cannot be completed.
pub(crate) fn initialize(
    panel_local_id: &str,
    update: impl FnOnce(&mut BrowserManifest),
) -> std::io::Result<BrowserManifest> {
    initialize_at(&default_manifest_path(panel_local_id), panel_local_id, update)
}

fn initialize_at(
    path: &Path,
    panel_local_id: &str,
    update: impl FnOnce(&mut BrowserManifest),
) -> std::io::Result<BrowserManifest> {
    mutate_at(path, panel_local_id, true, update)
}

/// Atomically read, mutate, and replace one manifest while holding the
/// inter-process manifest lock.
///
/// # Errors
/// Fails when the lock or manifest write cannot be completed.
pub fn update(panel_local_id: &str, update: impl FnOnce(&mut BrowserManifest)) -> std::io::Result<BrowserManifest> {
    update_at(&default_manifest_path(panel_local_id), panel_local_id, update)
}

fn update_at(
    path: &Path,
    panel_local_id: &str,
    update: impl FnOnce(&mut BrowserManifest),
) -> std::io::Result<BrowserManifest> {
    mutate_at(path, panel_local_id, false, update)
}

fn mutate_at(
    path: &Path,
    panel_local_id: &str,
    create_missing: bool,
    update: impl FnOnce(&mut BrowserManifest),
) -> std::io::Result<BrowserManifest> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire(path)?;
    let mut manifest = read_at(path)
        .or_else(|| {
            create_missing.then(|| BrowserManifest {
                panel_local_id: panel_local_id.to_string(),
                ..BrowserManifest::default()
            })
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("browser manifest no longer exists: {}", path.display()),
            )
        })?;
    update(&mut manifest);
    write_at_locked(path, &manifest)?;
    Ok(manifest)
}

fn write_at_locked(path: &Path, manifest: &BrowserManifest) -> std::io::Result<()> {
    let raw = serde_json::to_string_pretty(manifest).map_err(|e| std::io::Error::other(e.to_string()))?;
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(raw.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&temp, raw.as_bytes())?;
    replace_file(&temp, path)?;
    Ok(())
}

struct ManifestLock {
    path: PathBuf,
}

impl ManifestLock {
    fn acquire(manifest_path: &Path) -> std::io::Result<Self> {
        Self::acquire_with_stale_age(manifest_path, STALE_LOCK_AGE)
    }

    fn acquire_with_stale_age(manifest_path: &Path, stale_lock_age: Duration) -> std::io::Result<Self> {
        let path = manifest_path.with_extension("json.lock");
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(_file) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path, stale_lock_age) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for manifest lock {}", path.display()),
                        ));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for ManifestLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path, stale_lock_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
        .is_ok_and(|age| age >= stale_lock_age)
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
    let path = default_manifest_path(panel_local_id);
    remove_at_with_warning(&path);
}

/// Owns one driver's manifest from startup through every exit path.
pub(crate) struct DriverManifestLifetime {
    path: PathBuf,
}

impl DriverManifestLifetime {
    /// Remove any stale predecessor manifest and arm cleanup for this run.
    pub(crate) fn start(panel_local_id: &str) -> Self {
        Self::start_at(default_manifest_path(panel_local_id))
    }

    fn start_at(path: PathBuf) -> Self {
        remove_at_with_warning(&path);
        Self { path }
    }
}

impl Drop for DriverManifestLifetime {
    fn drop(&mut self) {
        remove_at_with_warning(&self.path);
    }
}

fn remove_at_with_warning(path: &Path) {
    if let Err(error) = remove_at(path, REMOVE_STALE_LOCK_AGE) {
        tracing::warn!(target: "browser", path = %path.display(), "failed to remove browser manifest: {error}");
    }
}

fn remove_at(path: &Path, stale_lock_age: Duration) -> std::io::Result<()> {
    let _lock = ManifestLock::acquire_with_stale_age(path, stale_lock_age)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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
    fn teardown_removal_reclaims_an_orphaned_lock() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "orphaned-lock");
        write_at(&path, &sample("orphaned-lock")).unwrap();
        let lock_path = path.with_extension("json.lock");
        std::fs::write(&lock_path, b"").unwrap();

        remove_at(&path, Duration::ZERO).unwrap();

        assert!(!path.exists());
        assert!(!lock_path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_is_sanitized() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "../evil");
        assert!(path.starts_with(&root));
        assert!(!path.to_string_lossy().contains(".."));
        assert_ne!(path, manifest_path_for_root(&root, "___evil"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn distinct_unsafe_ids_have_distinct_manifest_paths() {
        let root = test_root();

        assert_ne!(
            manifest_path_for_root(&root, "a/b"),
            manifest_path_for_root(&root, "a_b")
        );
        assert_ne!(manifest_path_for_root(&root, ""), manifest_path_for_root(&root, "_"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsafe_ids_list_as_the_original_id_and_reopen() {
        let root = test_root();
        let manifest_dir = root.join("runtime").join("browsers");
        let original_id = "../unsafe panel";
        let path = manifest_path_for_root(&root, original_id);
        write_at(&path, &sample(original_id)).unwrap();

        let listed = list_panels_in(&manifest_dir);

        assert_eq!(listed, [original_id]);
        assert_eq!(
            read_at(&manifest_path_for_root(&root, &listed[0])),
            Some(sample(original_id))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_ignores_a_manifest_with_a_mismatched_filename() {
        let root = test_root();
        let manifest_dir = root.join("runtime").join("browsers");
        let path = manifest_dir.join("wrong-id.json");
        write_at(&path, &sample("actual-id")).unwrap();

        assert!(list_panels_in(&manifest_dir).is_empty());
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
    fn user_activity_requires_a_fresh_timestamp() {
        let mut manifest = sample("active");
        manifest.user_active = true;
        manifest.user_active_at = 1_000;

        assert!(manifest.user_is_active(5_999));
        assert!(!manifest.user_is_active(6_001));
        manifest.user_active = false;
        assert!(!manifest.user_is_active(1_001));
    }

    #[test]
    fn concurrent_updates_preserve_independent_fields() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "race");
        write_at(&path, &sample("race")).unwrap();

        std::thread::scope(|scope| {
            let title_path = path.clone();
            scope.spawn(move || {
                for value in 0..50 {
                    update_at(&title_path, "race", |manifest| {
                        manifest.title = format!("title-{value}");
                    })
                    .unwrap();
                }
            });
            let activity_path = path.clone();
            scope.spawn(move || {
                for value in 0..50 {
                    update_at(&activity_path, "race", |manifest| {
                        manifest.user_active_at = value;
                    })
                    .unwrap();
                }
            });
        });

        let manifest = read_at(&path).unwrap();
        assert_eq!(manifest.title, "title-49");
        assert_eq!(manifest.user_active_at, 49);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_cannot_recreate_a_manifest_removed_while_waiting_for_its_lock() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "teardown-race");
        write_at(&path, &sample("teardown-race")).unwrap();
        let lock = ManifestLock::acquire(&path).unwrap();

        let result = std::thread::scope(|scope| {
            let update_path = path.clone();
            let update = scope.spawn(move || {
                update_at(&update_path, "teardown-race", |manifest| {
                    manifest.title = "late update".to_string();
                })
            });
            std::fs::remove_file(&path).unwrap();
            drop(lock);
            update.join().unwrap()
        });

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn driver_initialization_is_the_explicit_create_boundary() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "initialize");

        let manifest = initialize_at(&path, "initialize", |manifest| {
            manifest.browser_ws = "ws://127.0.0.1:2/devtools/browser/y".to_string();
        })
        .unwrap();

        assert_eq!(read_at(&path), Some(manifest));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn driver_lifetime_cleans_stale_and_new_manifests() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "lifetime");
        write_at(&path, &sample("lifetime")).unwrap();

        let lifetime = DriverManifestLifetime::start_at(path.clone());
        assert!(!path.exists());
        write_at(&path, &sample("lifetime")).unwrap();
        drop(lifetime);

        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handoff_lifecycle() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "p2");
        write_at(&path, &sample("p2")).unwrap();
        update_at(&path, "p2", |manifest| {
            manifest.handoff = Some(ManifestHandoff {
                request_id: "request-1".to_string(),
                reason: "captcha".to_string(),
                requested_at: now_millis(),
                done: false,
            });
        })
        .unwrap();
        assert!(read_at(&path).unwrap().handoff_pending().is_some());
        update_at(&path, "p2", |manifest| {
            manifest.handoff.as_mut().unwrap().done = true;
        })
        .unwrap();
        assert!(read_at(&path).unwrap().handoff_pending().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn handoff_request_ids_are_unique_for_same_millisecond_requests() {
        assert_ne!(new_handoff_request_id(), new_handoff_request_id());
    }
}
