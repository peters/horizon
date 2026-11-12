//! Durable identity for a keep-alive standalone browser host.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
use horizon_browser_control::process_info;
use horizon_browser_control::{atomic_file, manifest};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use super::StandaloneError;

const KEEP_ALIVE_IDLE: Duration = Duration::from_mins(1);
const KEEP_ALIVE_POLL: Duration = Duration::from_millis(100);
const STOP_GRACE: Duration = Duration::from_secs(15);
const STOP_ESCALATION: Duration = Duration::from_secs(3);
/// The time after `SIGKILL` that a keep-alive host is given to leave its lease.
const STOP_KILL: Duration = Duration::from_secs(3);

/// The three waits of a stop: the polite request, the termination signal and the kill.
/// A test gives shorter waits, and the code path of a stop stays the same one.
#[derive(Clone, Copy, Debug)]
pub(super) struct StopBounds {
    pub grace: Duration,
    pub escalation: Duration,
    pub kill: Duration,
}

impl Default for StopBounds {
    fn default() -> Self {
        Self {
            grace: STOP_GRACE,
            escalation: STOP_ESCALATION,
            kill: STOP_KILL,
        }
    }
}

/// Recorded keep-alive host a later MCP client or `resume` can reconnect to.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct StandaloneHostRef {
    /// Panel id published by the keep-alive host.
    pub panel_id: String,
    /// Process that owns the browser session.
    pub host_pid: u32,
    /// Unix timestamp in milliseconds when this host published its lease.
    #[serde(default)]
    pub created_at: i64,
    /// Process start identity used to reject PID reuse before signaling.
    #[serde(default)]
    pub start_identity: String,
    /// Resolved browser profile directory for this host.
    #[serde(default)]
    pub profile_dir: PathBuf,
}

impl StandaloneHostRef {
    pub(super) fn current(panel_id: String, profile_dir: PathBuf) -> Result<Self, StandaloneError> {
        let host_pid = std::process::id();
        let start_identity = process_start_identity(host_pid)
            .ok_or_else(|| StandaloneError::Startup("could not read host process start identity".to_string()))?;
        Ok(Self {
            panel_id,
            host_pid,
            created_at: now_millis(),
            start_identity,
            profile_dir,
        })
    }
}

/// Keep-alive lease published before the panel manifest can appear.
///
/// Dropping without [`Self::commit`] removes the lease and any orphaned
/// manifest so a failed startup cannot leak a dead panel.
pub(super) struct PendingLease {
    root: PathBuf,
    host: StandaloneHostRef,
    committed: bool,
}

impl PendingLease {
    pub(super) fn publish(root: &Path, panel_id: String, profile_dir: PathBuf) -> Result<Self, StandaloneError> {
        let host = StandaloneHostRef::current(panel_id, profile_dir)?;
        publish(root, &host)?;
        Ok(Self {
            root: root.to_path_buf(),
            host,
            committed: false,
        })
    }

    pub(super) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PendingLease {
    fn drop(&mut self) {
        if !self.committed {
            let _ = remove_host(&self.root, &self.host);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum KeepAliveEnd {
    Stopped,
    Idle,
}

pub(super) fn lease_path_for_root(root: &Path, panel_id: &str) -> PathBuf {
    manifest::manifest_path_for_root(root, panel_id).with_extension("lease.json")
}

pub(super) fn stop_path_for_root(root: &Path, panel_id: &str) -> PathBuf {
    manifest::manifest_path_for_root(root, panel_id).with_extension("stop")
}

pub(super) fn publish(root: &Path, host: &StandaloneHostRef) -> Result<(), StandaloneError> {
    let path = lease_path_for_root(root, &host.panel_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            StandaloneError::Startup(format!("could not create standalone lease directory: {error}"))
        })?;
    }
    let mut bytes = serde_json::to_vec_pretty(host)
        .map_err(|error| StandaloneError::Startup(format!("could not encode standalone lease: {error}")))?;
    bytes.push(b'\n');
    atomic_file::replace(&path, &bytes)
        .map_err(|error| StandaloneError::Startup(format!("could not write {}: {error}", path.display())))?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| StandaloneError::Startup(format!("could not secure {}: {error}", path.display())))?;
    Ok(())
}

pub(super) fn remove(root: &Path, panel_id: &str) {
    let _ = std::fs::remove_file(lease_path_for_root(root, panel_id));
    let _ = std::fs::remove_file(stop_path_for_root(root, panel_id));
}

fn remove_host(root: &Path, host: &StandaloneHostRef) -> Result<(), String> {
    if profile_is_allowed(root, &host.panel_id, &host.profile_dir) {
        match std::fs::remove_dir_all(&host.profile_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "could not remove profile {}: {error}",
                    host.profile_dir.display()
                ));
            }
        }
    }
    manifest::remove_in(root, &host.panel_id)
        .map_err(|error| format!("could not remove manifest for `{}`: {error}", host.panel_id))?;
    remove(root, &host.panel_id);
    Ok(())
}

fn profile_is_allowed(horizon_root: &Path, panel_id: &str, profile: &Path) -> bool {
    if profile.as_os_str().is_empty() {
        return false;
    }
    let manifest_path = manifest::manifest_path_for_root(horizon_root, panel_id);
    if profile.file_name() != manifest_path.file_stem() {
        return false;
    }
    allowed_profile_roots(horizon_root)
        .iter()
        .any(|root| profile.starts_with(root))
}

fn allowed_profile_roots(horizon_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![horizon_root.join("browser-profiles")];
    if let Some(home) = horizon_root.parent() {
        roots.push(home.join("Horizon").join("browser-profiles"));
    }
    roots
}

pub(super) fn request_stop(root: &Path, panel_id: &str) -> Result<(), String> {
    let path = stop_path_for_root(root, panel_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("could not create stop directory: {error}"))?;
    }
    std::fs::write(&path, b"stop").map_err(|error| format!("could not write {}: {error}", path.display()))
}

pub(super) fn live_host(root: &Path) -> Option<StandaloneHostRef> {
    prune_dead_at(root);
    list_leases(root)
        .into_iter()
        .filter(|host| host_is_current(host) && panel_manifest_exists(root, &host.panel_id))
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.panel_id.cmp(&right.panel_id))
        })
}

pub(super) fn reconnect(root: &Path, host: &StandaloneHostRef) -> Result<(), String> {
    prune_dead_at(root);
    let Some(live) = read_lease(root, &host.panel_id) else {
        return gone(&host.panel_id);
    };
    if live.host_pid == host.host_pid
        && live.start_identity == host.start_identity
        && host_is_current(&live)
        && panel_manifest_exists(root, &host.panel_id)
    {
        return Ok(());
    }
    gone(&host.panel_id)
}

pub(super) fn stop_hosts(root: &Path, panel_id: Option<&str>) -> Result<Vec<String>, String> {
    stop_hosts_within(root, panel_id, StopBounds::default())
}

/// [`stop_hosts`] with the waits of `bounds`.
///
/// A host that inherited "ignore `SIGTERM`" — the disposition of a background job of a
/// non-interactive shell, of `nohup`, or of a launcher — keeps that across `exec`, so
/// the termination signal of a stop never reaches it. `SIGKILL` cannot be ignored, and
/// the alternative is a browser host of a closed panel that runs on and holds its
/// profile.
pub(super) fn stop_hosts_within(
    root: &Path,
    panel_id: Option<&str>,
    bounds: StopBounds,
) -> Result<Vec<String>, String> {
    prune_dead_at(root);
    let targets = match panel_id {
        Some(panel_id) => {
            let host = read_lease(root, panel_id)
                .ok_or_else(|| format!("no keep-alive standalone host for panel `{panel_id}`"))?;
            vec![host]
        }
        None => list_leases(root),
    };
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let mut stopped = Vec::new();
    for host in &targets {
        request_stop(root, &host.panel_id)?;
        stopped.push(host.panel_id.clone());
    }
    wait_until_exited(&targets, bounds.grace);
    for host in &targets {
        if host_is_current(host) {
            let _ = signal_terminate(host.host_pid);
        }
    }
    wait_until_exited(&targets, bounds.escalation);
    // The termination signal reached a host that did not end on it, so this exact
    // process id alone is killed. `host_is_current` compares the start identity of the
    // process with the identity of the lease, so the signal never reaches a process that
    // took the id of a host that ended.
    for host in &targets {
        if host_is_current(host) {
            let _ = signal_kill(host.host_pid);
        }
    }
    wait_until_exited(&targets, bounds.kill);
    prune_dead_at(root);
    let remaining = targets
        .iter()
        .filter(|host| read_lease(root, &host.panel_id).is_some() || host_liveness(host) != HostLiveness::Stale)
        .map(|host| host.panel_id.clone())
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        Ok(stopped)
    } else {
        Err(format!(
            "keep-alive standalone host did not stop: {}",
            remaining.join(", ")
        ))
    }
}

fn wait_until_exited(hosts: &[StandaloneHostRef], grace: Duration) {
    let deadline = Instant::now() + grace;
    loop {
        if hosts.iter().all(|host| host_liveness(host) == HostLiveness::Stale) {
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(KEEP_ALIVE_POLL);
    }
}

pub(super) fn prune_dead_at(root: &Path) -> Vec<String> {
    let mut pruned = Vec::new();
    for host in list_leases(root) {
        if host_liveness(&host) != HostLiveness::Stale {
            continue;
        }
        if remove_host(root, &host).is_ok() {
            pruned.push(host.panel_id);
        }
    }
    pruned
}

fn list_leases(root: &Path) -> Vec<StandaloneHostRef> {
    let directory = root.join("runtime").join("browsers");
    let Ok(entries) = atomic_file::read_dir(&directory) else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if !name.ends_with(".lease.json") {
            continue;
        }
        if let Some(host) = decode_lease(root, &path) {
            hosts.push(host);
        }
    }
    hosts.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.panel_id.cmp(&right.panel_id))
    });
    hosts
}

fn read_lease(root: &Path, panel_id: &str) -> Option<StandaloneHostRef> {
    decode_lease(root, &lease_path_for_root(root, panel_id)).filter(|host| host.panel_id == panel_id)
}

fn decode_lease(root: &Path, path: &Path) -> Option<StandaloneHostRef> {
    let bytes = atomic_file::read(path).ok()?;
    let host = serde_json::from_slice::<StandaloneHostRef>(&bytes).ok()?;
    (lease_path_for_root(root, &host.panel_id) == path).then_some(host)
}

pub(super) async fn await_stop_at(root: &Path, panel_id: &str, poll: Duration) {
    let stop = stop_path_for_root(root, panel_id);
    loop {
        if stop.is_file() {
            return;
        }
        tokio::time::sleep(poll).await;
    }
}

pub(super) async fn await_keep_alive_at(root: &Path, panel_id: &str, idle: Duration, poll: Duration) -> KeepAliveEnd {
    let stop = stop_path_for_root(root, panel_id);
    let mut idle_since = None;
    loop {
        if stop.is_file() {
            return KeepAliveEnd::Stopped;
        }
        if owner_is_idle(root, panel_id) {
            let started = *idle_since.get_or_insert_with(std::time::Instant::now);
            if started.elapsed() >= idle {
                return KeepAliveEnd::Idle;
            }
        } else {
            idle_since = None;
        }
        tokio::time::sleep(poll).await;
    }
}

fn owner_is_idle(root: &Path, panel_id: &str) -> bool {
    let Some(manifest) = manifest::read_at(&manifest::manifest_path_for_root(root, panel_id)) else {
        return true;
    };
    manifest.live_owner(now_millis()).is_none()
}

fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostLiveness {
    Current,
    Stale,
    Unknown,
}

fn host_is_current(host: &StandaloneHostRef) -> bool {
    host_liveness(host) == HostLiveness::Current
}

fn host_liveness(host: &StandaloneHostRef) -> HostLiveness {
    if host.start_identity.is_empty() {
        return HostLiveness::Stale;
    }
    match pid_probe(host.host_pid) {
        PidProbe::Dead => HostLiveness::Stale,
        #[cfg(unix)]
        PidProbe::Unknown => HostLiveness::Unknown,
        PidProbe::Alive => {
            liveness_from_identity(process_start_identity(host.host_pid).as_deref(), &host.start_identity)
        }
    }
}

fn panel_manifest_exists(root: &Path, panel_id: &str) -> bool {
    manifest::read_at(&manifest::manifest_path_for_root(root, panel_id)).is_some()
}

fn gone(panel_id: &str) -> Result<(), String> {
    Err(format!(
        "standalone browser host for panel `{panel_id}` is gone and cannot be reconnected"
    ))
}

fn process_start_identity(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    platform_process_start_identity(pid)
}

/// Compare a live process identity with the one stored on a lease.
///
/// A different creation time means the pid was reused. Callers signal only a
/// current host, so the mismatched lease is pruned and the new process is left
/// alone.
fn liveness_from_identity(current: Option<&str>, stored: &str) -> HostLiveness {
    match current {
        Some(identity) if identity == stored => HostLiveness::Current,
        Some(_) => HostLiveness::Stale,
        None => HostLiveness::Unknown,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_stat_starttime(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)
}

#[cfg(target_os = "linux")]
fn platform_process_start_identity(pid: u32) -> Option<String> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let starttime = linux_stat_starttime(&stat)?;
    Some(format!("{}:{starttime}", boot.trim()))
}

#[cfg(target_os = "macos")]
fn platform_process_start_identity(pid: u32) -> Option<String> {
    let output = process_info::start_time_command(pid)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    process_info::parse_start_time(&output.stdout)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PidProbe {
    Alive,
    Dead,
    /// `kill -0` could not be spawned.
    #[cfg(unix)]
    Unknown,
}

fn pid_probe(pid: u32) -> PidProbe {
    if pid == 0 {
        return PidProbe::Dead;
    }
    #[cfg(unix)]
    {
        match Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => PidProbe::Alive,
            Ok(_) => PidProbe::Dead,
            Err(_) => PidProbe::Unknown,
        }
    }
}

fn signal_terminate(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args([&pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn signal_kill(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            // GNU kill reads a negative pid as an option, and the explicit signal name
            // keeps the argument one pid for every kill of this computer.
            .args(["-KILL", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

pub(crate) const fn keep_alive_idle() -> Duration {
    KEEP_ALIVE_IDLE
}

pub(crate) const fn keep_alive_poll() -> Duration {
    KEEP_ALIVE_POLL
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser_control::manifest::BrowserManifest;

    fn write_manifest(root: &Path, panel_id: &str) {
        manifest::write_at(
            &manifest::manifest_path_for_root(root, panel_id),
            &BrowserManifest {
                panel_local_id: panel_id.to_string(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"));
    }

    fn host(panel_id: &str, pid: u32, created_at: i64, identity: &str) -> StandaloneHostRef {
        StandaloneHostRef {
            panel_id: panel_id.to_string(),
            host_pid: pid,
            created_at,
            start_identity: identity.to_string(),
            profile_dir: PathBuf::new(),
        }
    }

    fn current_host(panel_id: &str, created_at: i64) -> StandaloneHostRef {
        let pid = std::process::id();
        let identity = process_start_identity(pid).unwrap_or_else(|| panic!("current process identity"));
        host(panel_id, pid, created_at, &identity)
    }

    #[test]
    fn linux_stat_starttime_uses_the_field_after_comm() {
        let stat = "1234 (chrome (helper)) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0";
        assert_eq!(linux_stat_starttime(stat), Some("98765"));
    }

    #[test]
    fn a_different_creation_time_is_not_the_same_host() {
        assert_eq!(
            liveness_from_identity(Some("2026-09-23T05:18:26.2182731Z"), "2026-09-23T05:18:26.2182731Z"),
            HostLiveness::Current
        );
        assert_eq!(
            liveness_from_identity(Some("2026-09-23T05:18:26.9000000Z"), "2026-09-23T05:18:26.2182731Z"),
            HostLiveness::Stale
        );
        assert_eq!(
            liveness_from_identity(None, "2026-09-23T05:18:26.2182731Z"),
            HostLiveness::Unknown
        );
    }

    #[test]
    fn current_process_start_identity_is_stable() {
        let pid = std::process::id();
        let first = process_start_identity(pid).unwrap_or_else(|| panic!("first identity"));
        let second = process_start_identity(pid).unwrap_or_else(|| panic!("second identity"));
        assert!(!first.is_empty());
        assert_eq!(first, second);
        assert!(host_is_current(&current_host("standalone-identity", 1)));
    }

    #[test]
    fn leases_are_ignored_when_the_embedded_panel_id_does_not_match_the_path() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let live = current_host("standalone-victim", 1);
        publish(home.path(), &live).unwrap_or_else(|error| panic!("publish live: {error}"));
        write_manifest(home.path(), &live.panel_id);
        let spoofed = host(&live.panel_id, 0, 1, "");
        let spoof_path = home
            .path()
            .join("runtime")
            .join("browsers")
            .join("standalone-spoofed.lease.json");
        std::fs::write(
            &spoof_path,
            serde_json::to_vec(&spoofed).unwrap_or_else(|error| panic!("encode spoof: {error}")),
        )
        .unwrap_or_else(|error| panic!("write spoof: {error}"));

        assert!(decode_lease(home.path(), &spoof_path).is_none());
        assert_eq!(prune_dead_at(home.path()), Vec::<String>::new());
        reconnect(home.path(), &live).unwrap_or_else(|error| panic!("live host must survive a spoofed lease: {error}"));
    }

    #[test]
    fn pending_lease_is_removed_with_its_manifest_unless_committed() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let abandoned = "standalone-9-abandoned";
        let pending = PendingLease::publish(home.path(), abandoned.to_string(), PathBuf::new())
            .unwrap_or_else(|error| panic!("publish pending: {error}"));
        write_manifest(home.path(), abandoned);
        drop(pending);
        assert!(read_lease(home.path(), abandoned).is_none());
        assert!(!panel_manifest_exists(home.path(), abandoned));

        let kept = "standalone-9-kept";
        PendingLease::publish(home.path(), kept.to_string(), PathBuf::new())
            .unwrap_or_else(|error| panic!("publish kept: {error}"))
            .commit();
        assert!(read_lease(home.path(), kept).is_some());
    }

    #[test]
    fn stop_waits_for_graceful_exit_before_signaling() {
        let live = current_host("standalone-9-grace", 1);
        wait_until_exited(&[live], Duration::from_millis(20));
        assert!(
            process_start_identity(std::process::id()).is_some(),
            "grace wait must not terminate the current process"
        );
    }

    #[test]
    fn dead_host_leases_are_pruned_with_their_manifests() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let mut recorded = host("standalone-9-dead", 0, 1, "");
        let profile = home.path().join("browser-profiles").join(
            manifest::manifest_path_for_root(home.path(), &recorded.panel_id)
                .file_stem()
                .unwrap_or_default(),
        );
        recorded.profile_dir = profile.clone();
        publish(home.path(), &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        write_manifest(home.path(), &recorded.panel_id);
        std::fs::create_dir_all(&profile).unwrap_or_else(|error| panic!("profile: {error}"));
        std::fs::write(profile.join("state"), b"left behind").unwrap_or_else(|error| panic!("profile file: {error}"));

        let pruned = prune_dead_at(home.path());
        assert_eq!(pruned, vec![recorded.panel_id.clone()]);
        assert!(read_lease(home.path(), &recorded.panel_id).is_none());
        assert!(!panel_manifest_exists(home.path(), &recorded.panel_id));
        assert!(!profile.exists());
    }

    #[test]
    fn failed_profile_cleanup_keeps_the_lease() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let mut recorded = host("standalone-9-locked", 0, 1, "");
        let profile = home.path().join("browser-profiles").join(
            manifest::manifest_path_for_root(home.path(), &recorded.panel_id)
                .file_stem()
                .unwrap_or_default(),
        );
        std::fs::create_dir_all(profile.parent().unwrap_or_else(|| panic!("profile parent")))
            .unwrap_or_else(|error| panic!("dir: {error}"));
        std::fs::write(&profile, b"not a directory").unwrap_or_else(|error| panic!("blocker: {error}"));
        recorded.profile_dir = profile;
        publish(home.path(), &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        write_manifest(home.path(), &recorded.panel_id);

        assert!(remove_host(home.path(), &recorded).is_err());
        assert!(read_lease(home.path(), &recorded.panel_id).is_some());
        assert!(panel_manifest_exists(home.path(), &recorded.panel_id));
    }

    #[test]
    fn dead_host_prune_removes_a_relocated_snap_profile() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let horizon = home.path().join(".horizon");
        let mut recorded = host("standalone-9-snap", 0, 1, "");
        let profile = home.path().join("Horizon").join("browser-profiles").join(
            manifest::manifest_path_for_root(&horizon, &recorded.panel_id)
                .file_stem()
                .unwrap_or_default(),
        );
        recorded.profile_dir = profile.clone();
        publish(&horizon, &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        write_manifest(&horizon, &recorded.panel_id);
        std::fs::create_dir_all(&profile).unwrap_or_else(|error| panic!("snap profile: {error}"));
        std::fs::write(profile.join("state"), b"snap").unwrap_or_else(|error| panic!("snap file: {error}"));

        assert_eq!(prune_dead_at(&horizon), vec![recorded.panel_id.clone()]);
        assert!(!profile.exists());
        assert!(read_lease(&horizon, &recorded.panel_id).is_none());
    }

    #[test]
    fn reused_pids_are_pruned_without_signaling() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let recorded = host("standalone-9-reused", std::process::id(), 1, "not-this-process");
        publish(home.path(), &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        write_manifest(home.path(), &recorded.panel_id);

        let pruned = prune_dead_at(home.path());
        assert_eq!(pruned, vec![recorded.panel_id.clone()]);
        assert!(read_lease(home.path(), &recorded.panel_id).is_none());
        assert!(!panel_manifest_exists(home.path(), &recorded.panel_id));
        assert!(
            process_start_identity(std::process::id()).is_some(),
            "test process must still be alive after refusing to signal a reused PID"
        );
    }

    #[test]
    fn live_host_selects_the_newest_created_lease() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let older = current_host("standalone-zzz-old", 1);
        let newer = current_host("standalone-aaa-new", 2);
        publish(home.path(), &older).unwrap_or_else(|error| panic!("publish older: {error}"));
        publish(home.path(), &newer).unwrap_or_else(|error| panic!("publish newer: {error}"));
        write_manifest(home.path(), &older.panel_id);
        write_manifest(home.path(), &newer.panel_id);

        assert_eq!(live_host(home.path()).as_ref(), Some(&newer));
    }

    #[test]
    fn reconnect_requires_matching_identity_and_manifest() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let recorded = current_host("standalone-9-live", 1);
        publish(home.path(), &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        assert!(reconnect(home.path(), &recorded).is_err());
        write_manifest(home.path(), &recorded.panel_id);
        reconnect(home.path(), &recorded).unwrap_or_else(|error| panic!("reconnect: {error}"));

        let reused = host(&recorded.panel_id, recorded.host_pid, recorded.created_at, "other");
        assert!(reconnect(home.path(), &reused).is_err());
    }

    /// A stand-in of a keep-alive host, which ignores the termination signal.
    ///
    /// The stand-in is reaped on a thread of its own, because the liveness of a lease
    /// reads `kill -0` and a zombie answers it: a stop cannot see the end of a host that
    /// nobody reaps. The drop kills the stand-in by its own process id, so a test that
    /// failed before the stop left no process behind.
    struct HostStandin {
        pid: u32,
        reaper: Option<std::thread::JoinHandle<std::io::Result<std::process::ExitStatus>>>,
    }

    impl HostStandin {
        /// A stand-in that inherited "ignore `SIGTERM`" as a background job does, so
        /// only the kill of the stop ends it.
        fn spawn() -> Self {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "trap '' TERM; while :; do sleep 0.02; done"])
                .spawn()
                .unwrap_or_else(|error| panic!("a host stand-in: {error}"));
            let pid = child.id();
            let reaper = std::thread::spawn(move || child.wait());
            Self {
                pid,
                reaper: Some(reaper),
            }
        }

        /// The status that the stand-in ended with, once it ended.
        fn ended(&mut self) -> std::process::ExitStatus {
            (self.reaper.take())
                .and_then(|reaper| reaper.join().ok())
                .and_then(std::io::Result::ok)
                .unwrap_or_else(|| panic!("the stand-in of a host does not end"))
        }
    }

    impl Drop for HostStandin {
        fn drop(&mut self) {
            let _ = signal_kill(self.pid);
            if let Some(reaper) = self.reaper.take() {
                let _ = reaper.join();
            }
        }
    }

    #[test]
    fn a_host_that_ignores_the_termination_signal_ends_by_the_kill_of_the_stop() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let panel_id = "standalone-9-kill";
        write_manifest(home.path(), panel_id);
        let mut host_process = HostStandin::spawn();
        let identity =
            process_start_identity(host_process.pid).unwrap_or_else(|| panic!("the identity of the stand-in"));
        publish(home.path(), &host(panel_id, host_process.pid, 1, &identity))
            .unwrap_or_else(|error| panic!("publish: {error}"));

        let stopped = stop_hosts_within(
            home.path(),
            Some(panel_id),
            StopBounds {
                grace: Duration::from_millis(100),
                escalation: Duration::from_millis(200),
                kill: Duration::from_secs(5),
            },
        )
        .unwrap_or_else(|error| panic!("stop: {error}"));

        assert_eq!(stopped, vec![panel_id.to_string()]);
        let ended = host_process.ended();
        assert!(
            !ended.success(),
            "the stand-in ended on the kill of the stop and not by itself"
        );
    }

    #[tokio::test]
    async fn keep_alive_ends_when_stop_is_requested() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let panel_id = "standalone-9-stop";
        std::fs::create_dir_all(home.path().join("runtime").join("browsers"))
            .unwrap_or_else(|error| panic!("dir: {error}"));
        let wait = tokio::spawn({
            let root = home.path().to_path_buf();
            async move { await_keep_alive_at(&root, panel_id, Duration::from_secs(30), Duration::from_millis(10)).await }
        });
        request_stop(home.path(), panel_id).unwrap_or_else(|error| panic!("stop: {error}"));
        assert_eq!(
            wait.await.unwrap_or_else(|error| panic!("join: {error}")),
            KeepAliveEnd::Stopped
        );
    }

    #[tokio::test]
    async fn keep_alive_idles_without_a_live_owner() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let panel_id = "standalone-9-idle";
        let ended = await_keep_alive_at(
            home.path(),
            panel_id,
            Duration::from_millis(20),
            Duration::from_millis(5),
        )
        .await;
        assert_eq!(ended, KeepAliveEnd::Idle);
    }
}
