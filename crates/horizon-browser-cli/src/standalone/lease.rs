//! Durable identity for a keep-alive standalone browser host.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use horizon_core::browser::manifest;

use super::StandaloneError;

const KEEP_ALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEP_ALIVE_POLL: Duration = Duration::from_millis(100);

/// Recorded keep-alive host a later MCP client or `resume` can reconnect to.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct StandaloneHostRef {
    /// Panel id published by the keep-alive host.
    pub panel_id: String,
    /// Process that owns the browser session.
    pub host_pid: u32,
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
        .filter(|host| {
            pid_is_alive(host.host_pid)
                && manifest::read_at(&manifest::manifest_path_for_root(root, &host.panel_id)).is_some()
        })
        .min_by(|left, right| left.panel_id.cmp(&right.panel_id))
}

pub(super) fn reconnect(root: &Path, host: &StandaloneHostRef) -> Result<(), String> {
    prune_dead_at(root);
    if pid_is_alive(host.host_pid)
        && manifest::read_at(&manifest::manifest_path_for_root(root, &host.panel_id)).is_some()
        && read_lease(root, &host.panel_id).is_some_and(|live| live.host_pid == host.host_pid)
    {
        return Ok(());
    }
    Err(format!(
        "standalone browser host for panel `{}` is gone and cannot be reconnected",
        host.panel_id
    ))
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
    for host in targets {
        request_stop(root, &host.panel_id)?;
        if pid_is_alive(host.host_pid) {
            let _ = signal_terminate(host.host_pid);
        }
        stopped.push(host.panel_id);
    }
    Ok(stopped)
}

pub(super) fn prune_dead_at(root: &Path) -> Vec<String> {
    let mut pruned = Vec::new();
    for host in list_leases(root) {
        if pid_is_alive(host.host_pid) {
            continue;
        }
        let _ = std::fs::remove_file(manifest::manifest_path_for_root(root, &host.panel_id));
        remove(root, &host.panel_id);
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
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(host) = serde_json::from_slice::<StandaloneHostRef>(&bytes)
        {
            hosts.push(host);
        }
    }
    hosts.sort_by(|left, right| left.panel_id.cmp(&right.panel_id));
    hosts
}

fn read_lease(root: &Path, panel_id: &str) -> Option<StandaloneHostRef> {
    let bytes = std::fs::read(lease_path_for_root(root, panel_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
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

pub(super) fn pid_is_alive(pid: u32) -> bool {
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
    use horizon_browser::BackendKind;
    use horizon_core::browser::manifest::BrowserManifest;

    #[cfg(unix)]
    #[test]
    fn dead_host_leases_are_pruned_with_their_manifests() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let mut child = Command::new("/bin/true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn: {error}"));
        let pid = child.id();
        let _ = child.wait();
        let host = StandaloneHostRef {
            panel_id: "standalone-9-dead".to_string(),
            host_pid: pid,
        };
        publish(home.path(), &host).unwrap_or_else(|error| panic!("publish: {error}"));
        manifest::write_at(
            &manifest::manifest_path_for_root(home.path(), &host.panel_id),
            &BrowserManifest {
                panel_local_id: host.panel_id.clone(),
                backend: BackendKind::ChromiumCdp,
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"));

        let pruned = prune_dead_at(home.path());
        assert_eq!(pruned, vec![host.panel_id.clone()]);
        assert!(read_lease(home.path(), &host.panel_id).is_none());
        assert!(manifest::read_at(&manifest::manifest_path_for_root(home.path(), &host.panel_id)).is_none());
    }

    #[test]
    fn reconnect_requires_a_live_pid_and_manifest() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("home: {error}"));
        let host = StandaloneHostRef {
            panel_id: "standalone-9-live".to_string(),
            host_pid: std::process::id(),
        };
        publish(home.path(), &host).unwrap_or_else(|error| panic!("publish: {error}"));
        assert!(reconnect(home.path(), &host).is_err());
        manifest::write_at(
            &manifest::manifest_path_for_root(home.path(), &host.panel_id),
            &BrowserManifest {
                panel_local_id: host.panel_id.clone(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"));
        reconnect(home.path(), &host).unwrap_or_else(|error| panic!("reconnect: {error}"));
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
