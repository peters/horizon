//! File-based ownership/handoff manifest for browser panels.
//!
//! Each browser panel writes a small JSON file under
//! `~/.horizon/runtime/browsers/<encoded-panel-local-id>.json` while it is
//! alive.
//! It is the private shared channel between three parties:
//!
//! - the **panel driver** (writes backend endpoint/context, `url`,
//!   `title`, `user_active`, and the handoff `done` flag),
//! - the **Horizon MCP adapter** (reads discovery fields, heartbeats `owner`,
//!   writes a `handoff` request, and enqueues validated backend-neutral
//!   actions),
//! - the **UI host** (reads ownership for the chrome chip and stamps the
//!   host-owned `hidden` and `workspace` fields).
//!
//! File-based on purpose: it works across process boundaries without putting
//! a transport dependency in `horizon-browser`. It is not a supported agent
//! API; MCP is the sole agent-facing browser contract.
//! Every update to an existing manifest must use [`update`] (or implement its
//! adjacent `<panel>.json.lock` protocol around both the read and the write).
//! The lock file is a stable coordination inode guarded by the operating
//! system, so a crashed holder releases the lock without any stale-file
//! deletion race. This serializes read-modify-write transactions across the
//! driver and agent processes so ownership/handoff fields cannot be lost.
//! Driver startup uses [`initialize`] to create the manifest explicitly;
//! later updates fail when teardown has removed it, so a delayed writer cannot
//! resurrect a dead panel.

use std::fs::{OpenOptions, TryLockError};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};

use crate::horizon_home::{HorizonHome, safe_local_id};

mod agent;
mod audit;
mod capture;
mod create;
mod request_queue;
mod result;
mod visibility;
mod workspace;

pub use agent::{claim, enqueue_action, heartbeat, release, request_handoff};
pub use audit::{audit_path_for_root, default_audit_path, read_audit};
pub use create::{
    BrowserCreateAuditStatus, BrowserCreateOutcome, BrowserCreateRequest, BrowserCreateResult, claim_create_request,
    complete_create_request, enqueue_create, list_create_requests, record_create_status, take_create_result,
};
pub use result::{action_result_path_for_root, default_action_result_path, take_action_result};
pub use visibility::{
    BrowserVisibilityAuditStatus, BrowserVisibilityOutcome, BrowserVisibilityRequest, BrowserVisibilityResult,
    claim_visibility_request, complete_visibility_request, enqueue_visibility, list_visibility_requests,
    record_visibility_status, take_visibility_result,
};
pub use workspace::{ManifestWorkspace, sync_host_state};

/// How long an agent owner heartbeat stays fresh.
pub const OWNER_TTL_MILLIS: i64 = 10_000;
/// How long the driver's `user_active` signal stays true without another
/// qualifying interaction.
pub const USER_ACTIVE_TTL: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const LOCK_WAIT: Duration = Duration::from_secs(2);
// Concurrency tests intentionally serialize many fsync-backed replacements;
// their purpose is transaction correctness, not a loaded runner's disk speed.
#[cfg(test)]
const LOCK_WAIT: Duration = Duration::from_secs(10);
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
    #[serde(default)]
    pub backend: horizon_browser::BackendKind,
    /// Negotiated CDP/BiDi WebSocket endpoint, or empty for classic-only
    /// Safari. The MCP adapter uses the validated action queue instead.
    pub browser_ws: String,
    /// The page target or top-level browsing context the panel is driving.
    pub target_id: String,
    pub url: String,
    pub title: String,
    /// Host-owned presentation state. Hidden panels keep their browser
    /// session and control channel alive.
    #[serde(default)]
    pub hidden: bool,
    /// Host-owned workspace membership stamped by the Horizon process hosting
    /// the panel. Absent until the host stamps it and on manifests from
    /// older hosts; workspace-scoped callers must treat both as out of scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<ManifestWorkspace>,
    /// Private append-only JSONL action journal for this panel identity.
    #[serde(default)]
    pub audit_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ManifestOwner>,
    pub user_active: bool,
    pub user_active_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<ManifestHandoff>,
    /// Bounded host queue consumed only while the user is not steering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<horizon_browser::AgentAction>,
    pub updated_at: i64,
}

impl BrowserManifest {
    /// The owner, if it has heartbeat within [`OWNER_TTL_MILLIS`].
    #[must_use]
    pub fn live_owner(&self, now_millis: i64) -> Option<&ManifestOwner> {
        self.owner
            .as_ref()
            .filter(|owner| timestamp_is_fresh(now_millis, owner.updated_at, OWNER_TTL_MILLIS))
    }

    #[must_use]
    pub fn handoff_pending(&self) -> Option<&ManifestHandoff> {
        self.handoff.as_ref().filter(|h| !h.done)
    }

    /// Whether the user-activity signal is both asserted and fresh.
    #[must_use]
    pub fn user_is_active(&self, now_millis: i64) -> bool {
        let ttl_millis = i64::try_from(USER_ACTIVE_TTL.as_millis()).unwrap_or(i64::MAX);
        self.user_active && timestamp_is_fresh(now_millis, self.user_active_at, ttl_millis)
    }

    /// Whether the host has placed `actor` in this panel's workspace. An
    /// unstamped manifest authorizes nobody.
    #[must_use]
    pub fn authorizes_actor(&self, actor: &str) -> bool {
        self.workspace
            .as_ref()
            .is_some_and(|workspace| workspace.authorizes(actor))
    }
}

fn timestamp_is_fresh(now_millis: i64, timestamp_millis: i64, ttl_millis: i64) -> bool {
    let age_millis = now_millis.saturating_sub(timestamp_millis);
    (0..=ttl_millis).contains(&age_millis)
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
    manifest_path_for_root(HorizonHome::resolve().root(), panel_local_id)
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
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| file.write_all(raw.as_bytes()), options)
        .map_err(Into::into)
}

struct ManifestLock {
    // The operating system releases the advisory lock when this handle is
    // dropped, including after a process crash. The coordination file itself
    // intentionally remains stable so contenders can never lock different
    // inodes for the same manifest.
    _file: std::fs::File,
}

impl ManifestLock {
    fn acquire(manifest_path: &Path) -> std::io::Result<Self> {
        Self::acquire_with_timeout(manifest_path, LOCK_WAIT)
    }

    fn acquire_with_timeout(manifest_path: &Path, timeout: Duration) -> std::io::Result<Self> {
        let path = manifest_path.with_extension("json.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for manifest lock {}", path.display()),
                        ));
                    }
                    std::thread::sleep(remaining.min(LOCK_RETRY));
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for manifest lock {}", path.display()),
                        ));
                    }
                }
                Err(TryLockError::Error(error)) => return Err(error),
            }
        }
    }
}

pub fn remove(panel_local_id: &str) {
    let path = default_manifest_path(panel_local_id);
    remove_at_with_warning(&path);
}

/// Remove a live manifest within the caller's remaining shutdown budget.
/// Returns `false` if lock acquisition or removal cannot finish in time.
#[must_use]
pub(crate) fn remove_with_timeout(panel_local_id: &str, timeout: Duration) -> bool {
    let path = default_manifest_path(panel_local_id);
    match remove_at_with_timeout(&path, timeout) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(target: "browser", path = %path.display(), "failed to remove browser manifest: {error}");
            false
        }
    }
}

/// Owns one driver's manifest from startup through every exit path.
#[cfg(test)]
pub(crate) struct DriverManifestLifetime {
    path: PathBuf,
}

#[cfg(test)]
impl DriverManifestLifetime {
    fn start_at(path: PathBuf) -> Self {
        remove_at_with_warning(&path);
        Self { path }
    }
}

#[cfg(test)]
impl Drop for DriverManifestLifetime {
    fn drop(&mut self) {
        remove_at_with_warning(&self.path);
    }
}

fn remove_at_with_warning(path: &Path) {
    if let Err(error) = remove_at(path) {
        tracing::warn!(target: "browser", path = %path.display(), "failed to remove browser manifest: {error}");
    }
}

fn remove_at(path: &Path) -> std::io::Result<()> {
    remove_at_with_timeout(path, LOCK_WAIT)
}

fn remove_at_with_timeout(path: &Path, timeout: Duration) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire_with_timeout(path, timeout)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Horizon's filesystem-backed implementation of the engine's optional live
/// coordination boundary.
#[derive(Debug, Default)]
pub struct ManifestCoordination {
    audit: audit::AuditSink,
}

impl horizon_browser::BrowserCoordination for ManifestCoordination {
    fn prepare(&self, panel_local_id: &str, timeout: Duration) -> bool {
        let manifest_removed = remove_with_timeout(panel_local_id, timeout);
        let results_removed = result::remove_stale(panel_local_id).is_ok();
        manifest_removed && results_removed
    }

    fn initialize(&self, panel_local_id: &str, state: &horizon_browser::CoordinationState) -> std::io::Result<()> {
        initialize(panel_local_id, |manifest| {
            manifest.panel_local_id = panel_local_id.to_string();
            manifest.backend = state.backend;
            manifest.browser_ws.clone_from(&state.browser_ws);
            manifest.target_id.clone_from(&state.target_id);
            manifest.url.clone_from(&state.url);
            manifest.title.clone_from(&state.title);
            manifest.audit_path = default_audit_path(panel_local_id).to_string_lossy().to_string();
            manifest.user_active = false;
            manifest.user_active_at = 0;
            manifest.updated_at = now_millis();
        })
        .map(|_| ())
    }

    fn update(&self, panel_local_id: &str, state: &horizon_browser::CoordinationState) -> std::io::Result<()> {
        update(panel_local_id, |manifest| {
            manifest.backend = state.backend;
            manifest.browser_ws.clone_from(&state.browser_ws);
            manifest.target_id.clone_from(&state.target_id);
            manifest.url.clone_from(&state.url);
            manifest.title.clone_from(&state.title);
            manifest.updated_at = now_millis();
        })
        .map(|_| ())
    }

    fn set_user_active(&self, panel_local_id: &str, active: bool) -> std::io::Result<()> {
        update(panel_local_id, |manifest| {
            manifest.user_active = active;
            manifest.user_active_at = now_millis();
            manifest.updated_at = manifest.user_active_at;
        })
        .map(|_| ())
    }

    fn signals(&self, panel_local_id: &str) -> std::io::Result<horizon_browser::CoordinationSignals> {
        let Some(snapshot) = read(panel_local_id) else {
            return Ok(horizon_browser::CoordinationSignals::default());
        };
        let now = now_millis();
        let legacy_handoff = snapshot
            .handoff_pending()
            .is_some_and(|handoff| handoff.request_id.is_empty());
        let actions_can_advance =
            !snapshot.actions.is_empty() && !snapshot.user_is_active(now) && snapshot.handoff_pending().is_none();
        if !legacy_handoff && !actions_can_advance {
            return Ok(signals_from_manifest(&snapshot, Vec::new()));
        }
        let generated_request_id = new_handoff_request_id();
        let mut actions = Vec::new();
        let mut rejected = Vec::new();
        let manifest = update(panel_local_id, |current| {
            if let Some(current_handoff) = current.handoff.as_mut()
                && !current_handoff.done
                && current_handoff.request_id.is_empty()
            {
                current_handoff.request_id.clone_from(&generated_request_id);
            }
            (actions, rejected) = agent::take_ready_actions(current);
        })?;
        if let Err(error) = agent::append_rejected_actions(panel_local_id, rejected) {
            tracing::warn!(target: "browser", "failed to append rejected-action audit: {error}");
        }
        Ok(signals_from_manifest(&manifest, actions))
    }

    fn acknowledge_handoff(&self, panel_local_id: &str, request_id: &str) -> std::io::Result<bool> {
        let mut acknowledged = false;
        update(panel_local_id, |manifest| {
            if let Some(handoff) = manifest
                .handoff
                .as_mut()
                .filter(|handoff| !handoff.done && handoff.request_id == request_id)
            {
                handoff.done = true;
                acknowledged = true;
            }
        })?;
        Ok(acknowledged)
    }

    fn record_action(&self, panel_local_id: &str, entry: &horizon_browser::BrowserAuditEntry) -> std::io::Result<()> {
        self.audit.append(entry, panel_local_id)
    }

    fn complete_action(
        &self,
        panel_local_id: &str,
        result: &horizon_browser::AgentActionResult,
    ) -> std::io::Result<()> {
        result::write(panel_local_id, result)
    }

    fn prepare_network_capture(
        &self,
        panel_local_id: &str,
        directory: &Path,
        requested_max_file_bytes: u64,
    ) -> std::io::Result<()> {
        capture::prepare(panel_local_id, directory, requested_max_file_bytes)
    }

    fn remove(&self, panel_local_id: &str, timeout: Duration) -> bool {
        let manifest_removed = remove_with_timeout(panel_local_id, timeout);
        let results_removed = result::remove_stale(panel_local_id).is_ok();
        manifest_removed && results_removed
    }
}

fn signals_from_manifest(
    manifest: &BrowserManifest,
    actions: Vec<horizon_browser::AgentAction>,
) -> horizon_browser::CoordinationSignals {
    let owner = manifest.live_owner(now_millis()).map(|owner| owner.name.clone());
    let handoff = manifest.handoff_pending().and_then(|handoff| {
        (!handoff.request_id.is_empty()).then(|| horizon_browser::HandoffRequest {
            request_id: handoff.request_id.clone(),
            reason: handoff.reason.clone(),
        })
    });
    horizon_browser::CoordinationSignals {
        owner,
        handoff,
        actions,
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
            backend: horizon_browser::BackendKind::ChromiumCdp,
            browser_ws: "ws://127.0.0.1:1/devtools/browser/x".to_string(),
            target_id: "T1".to_string(),
            url: "https://example.com".to_string(),
            title: "Example".to_string(),
            hidden: false,
            workspace: None,
            audit_path: String::new(),
            owner: None,
            user_active: false,
            user_active_at: 0,
            handoff: None,
            actions: Vec::new(),
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
    fn teardown_removal_reuses_an_unlocked_coordination_file() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "orphaned-lock");
        write_at(&path, &sample("orphaned-lock")).unwrap();
        let lock_path = path.with_extension("json.lock");
        std::fs::write(&lock_path, b"").unwrap();

        remove_at(&path).unwrap();

        assert!(!path.exists());
        assert!(lock_path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn teardown_removal_honors_the_callers_lock_deadline() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "bounded-removal");
        write_at(&path, &sample("bounded-removal")).unwrap();
        let lock = ManifestLock::acquire(&path).unwrap();
        let started = Instant::now();

        let error = remove_at_with_timeout(&path, Duration::from_millis(20)).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(path.exists());
        drop(lock);
        remove_at(&path).unwrap();
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
        assert_ne!(
            manifest_path_for_root(&root, "Panel-A"),
            manifest_path_for_root(&root, "panel-a")
        );
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
            updated_at: now,
        });
        write_at(&path, &m).unwrap();
        let read_back = read_at(&path).unwrap();
        assert!(read_back.live_owner(now).is_some());
        assert!(read_back.live_owner(now + OWNER_TTL_MILLIS).is_some());
        assert!(read_back.live_owner(now + OWNER_TTL_MILLIS + 1).is_none());
        assert!(read_back.live_owner(now - 1).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_activity_requires_a_fresh_timestamp() {
        let mut manifest = sample("active");
        manifest.user_active = true;
        manifest.user_active_at = 1_000;

        assert!(manifest.user_is_active(5_999));
        assert!(manifest.user_is_active(6_000));
        assert!(!manifest.user_is_active(6_001));
        assert!(!manifest.user_is_active(999));
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
    fn removing_a_missing_manifest_initializes_its_coordination_directory() {
        let root = test_root();
        let path = manifest_path_for_root(&root, "missing");

        remove_at(&path).unwrap();

        assert!(!path.exists());
        assert!(path.with_extension("json.lock").exists());
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
