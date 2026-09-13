//! Durable identity for a keep-alive standalone browser host.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use horizon_core::browser::manifest;

use super::StandaloneError;

const KEEP_ALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEP_ALIVE_POLL: Duration = Duration::from_millis(100);
const STOP_GRACE: Duration = Duration::from_secs(15);
const STOP_ESCALATION: Duration = Duration::from_secs(3);

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
}

impl StandaloneHostRef {
    pub(super) fn current(panel_id: String) -> Result<Self, StandaloneError> {
        let host_pid = std::process::id();
        let start_identity = process_start_identity(host_pid)
            .ok_or_else(|| StandaloneError::Startup("could not read host process start identity".to_string()))?;
        Ok(Self {
            panel_id,
            host_pid,
            created_at: now_millis(),
            start_identity,
        })
    }
}

/// Keep-alive lease published before the panel manifest can appear.
///
/// Dropping without [`Self::commit`] removes the lease and any orphaned
/// manifest so a failed startup cannot leak a dead panel.
pub(super) struct PendingLease {
    root: PathBuf,
    panel_id: String,
    committed: bool,
}

impl PendingLease {
    pub(super) fn publish(root: &Path, panel_id: String) -> Result<Self, StandaloneError> {
        publish(root, &StandaloneHostRef::current(panel_id.clone())?)?;
        Ok(Self {
            root: root.to_path_buf(),
            panel_id,
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
            remove_host(&self.root, &self.panel_id);
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
    let bytes = serde_json::to_vec_pretty(host)
        .map_err(|error| StandaloneError::Startup(format!("could not encode standalone lease: {error}")))?;
    std::fs::write(&path, bytes)
        .map_err(|error| StandaloneError::Startup(format!("could not write {}: {error}", path.display())))?;
    Ok(())
}

pub(super) fn remove(root: &Path, panel_id: &str) {
    let _ = std::fs::remove_file(lease_path_for_root(root, panel_id));
    let _ = std::fs::remove_file(stop_path_for_root(root, panel_id));
}

pub(super) fn remove_host(root: &Path, panel_id: &str) {
    let manifest_path = manifest::manifest_path_for_root(root, panel_id);
    if let Some(encoded) = manifest_path.file_stem() {
        let _ = std::fs::remove_dir_all(root.join("browser-profiles").join(encoded));
    }
    let _ = std::fs::remove_file(manifest_path);
    remove(root, panel_id);
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
    wait_until_exited(&targets, STOP_GRACE);
    for host in &targets {
        if host_is_current(host) {
            let _ = signal_terminate(host.host_pid);
        }
    }
    wait_until_exited(&targets, STOP_ESCALATION);
    prune_dead_at(root);
    let survivors = targets
        .iter()
        .filter(|host| host_is_current(host))
        .map(|host| host.panel_id.clone())
        .collect::<Vec<_>>();
    if survivors.is_empty() {
        Ok(stopped)
    } else {
        Err(format!(
            "keep-alive standalone host did not stop: {}",
            survivors.join(", ")
        ))
    }
}

fn wait_until_exited(hosts: &[StandaloneHostRef], grace: Duration) {
    let deadline = Instant::now() + grace;
    loop {
        if hosts.iter().all(|host| !host_is_current(host)) {
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
        if host_is_current(&host) {
            continue;
        }
        remove_host(root, &host.panel_id);
        pruned.push(host.panel_id);
    }
    pruned
}

fn list_leases(root: &Path) -> Vec<StandaloneHostRef> {
    let directory = root.join("runtime").join("browsers");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    for entry in entries.flatten() {
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
    let bytes = std::fs::read(path).ok()?;
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

fn host_is_current(host: &StandaloneHostRef) -> bool {
    !host.start_identity.is_empty()
        && pid_is_alive(host.host_pid)
        && process_start_identity(host.host_pid).as_deref() == Some(host.start_identity.as_str())
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
    command_identity(
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?,
    )
}

#[cfg(windows)]
fn platform_process_start_identity(pid: u32) -> Option<String> {
    command_identity(
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.ToUniversalTime().ToString('o')"
                ),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_process_start_identity(_pid: u32) -> Option<String> {
    None
}

#[cfg(any(target_os = "macos", windows))]
fn command_identity(output: std::process::Output) -> Option<String> {
    if !output.status.success() {
        return None;
    }
    let identity = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('\u{feff}')
        .trim()
        .to_string();
    if identity.is_empty() { None } else { Some(identity) }
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
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
    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
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
    use horizon_core::browser::manifest::BrowserManifest;

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
        let pending = PendingLease::publish(home.path(), abandoned.to_string())
            .unwrap_or_else(|error| panic!("publish pending: {error}"));
        write_manifest(home.path(), abandoned);
        drop(pending);
        assert!(read_lease(home.path(), abandoned).is_none());
        assert!(!panel_manifest_exists(home.path(), abandoned));

        let kept = "standalone-9-kept";
        PendingLease::publish(home.path(), kept.to_string())
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
        let recorded = host("standalone-9-dead", 0, 1, "");
        publish(home.path(), &recorded).unwrap_or_else(|error| panic!("publish: {error}"));
        write_manifest(home.path(), &recorded.panel_id);
        let profile = home.path().join("browser-profiles").join(
            manifest::manifest_path_for_root(home.path(), &recorded.panel_id)
                .file_stem()
                .unwrap_or_default(),
        );
        std::fs::create_dir_all(&profile).unwrap_or_else(|error| panic!("profile: {error}"));
        std::fs::write(profile.join("state"), b"left behind").unwrap_or_else(|error| panic!("profile file: {error}"));

        let pruned = prune_dead_at(home.path());
        assert_eq!(pruned, vec![recorded.panel_id.clone()]);
        assert!(read_lease(home.path(), &recorded.panel_id).is_none());
        assert!(!panel_manifest_exists(home.path(), &recorded.panel_id));
        assert!(!profile.exists());
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
