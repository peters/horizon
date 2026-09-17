//! Bounded host-scoped requests for retired remote allocations.
use super::request_queue::{
    MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json,
};
use super::{AgentIdentity, ManifestLock};
use crate::paths::{BrowserRuntimePaths, safe_local_id};
use horizon_browser::RemoteRecoveryStatus;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoteAllocationSummary {
    pub reference: String,
    pub provider: String,
    pub status: RemoteRecoveryStatus,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecoveryRequest {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub reference: Option<String>,
    pub deadline_at_millis: i64,
    claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecoveryResult {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub allocations: Vec<RemoteAllocationSummary>,
    pub error: Option<String>,
}

impl RecoveryRequest {
    #[must_use]
    pub fn result(&self, allocations: Vec<RemoteAllocationSummary>, error: Option<String>) -> RecoveryResult {
        RecoveryResult {
            request_id: self.request_id.clone(),
            actor: self.actor.clone(),
            host_instance: self.host_instance.clone(),
            allocations,
            error,
        }
    }
}

/// # Errors
/// Invalid host identity, a full queue, or unavailable private storage.
pub fn enqueue_recovery(identity: AgentIdentity<'_>, reference: Option<String>) -> std::io::Result<String> {
    enqueue_at(BrowserRuntimePaths::resolve().root(), identity, reference)
}

fn enqueue_at(root: &Path, identity: AgentIdentity<'_>, reference: Option<String>) -> std::io::Result<String> {
    super::agent::validate_actor(identity.actor)?;
    let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
    if !identity.workspace_scoped()
        || host.is_none()
        || reference
            .as_ref()
            .is_some_and(|r| r.is_empty() || r.len() > 128 || r.chars().any(char::is_control))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "recovery requires a Horizon host identity and a valid allocation reference",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if request_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "recovery queue is full",
        ));
    }
    let request = RecoveryRequest {
        request_id: horizon_browser::new_action_id(),
        actor: identity.actor.to_string(),
        host_instance: host.unwrap_or_default().to_string(),
        reference,
        deadline_at_millis: super::now_millis() + 15_000,
        claimed: false,
    };
    write_private_json(&path(root, &request.request_id, "request"), &request)?;
    Ok(request.request_id)
}

/// Claim only this host's requests atomically. Host dispatch must verify the
/// live actor and the retained allocation's scope before any disclosure.
/// # Errors
/// Unavailable or malformed queue storage.
pub fn claim_recovery_requests(host: &str) -> std::io::Result<Vec<RecoveryRequest>> {
    claim_at(BrowserRuntimePaths::resolve().root(), host)
}

fn claim_at(root: &Path, host: &str) -> std::io::Result<Vec<RecoveryRequest>> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    let mut requests = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let Some(mut request) = read_json::<RecoveryRequest>(&entry.path())? else {
            continue;
        };
        if request.host_instance != host
            || request.claimed
            || entry.path() != path(root, &request.request_id, "request")
        {
            continue;
        }
        request.claimed = true;
        write_private_json(&entry.path(), &request)?;
        requests.push(request);
    }
    Ok(requests)
}

/// # Errors
/// Private result storage is unavailable.
pub fn complete_recovery(result: &RecoveryResult) -> std::io::Result<()> {
    complete_at(BrowserRuntimePaths::resolve().root(), result)
}

fn complete_at(root: &Path, result: &RecoveryResult) -> std::io::Result<()> {
    let dir = directory(root);
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    let request_path = path(root, &result.request_id, "request");
    let Some(request) = read_json::<RecoveryRequest>(&request_path)? else {
        return Ok(());
    };
    if !request.claimed || request.actor != result.actor || request.host_instance != result.host_instance {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "recovery result identity mismatch",
        ));
    }
    write_private_json(&path(root, &result.request_id, "result"), result)?;
    std::fs::remove_file(request_path)
}

/// # Errors
/// Result storage is unavailable or does not match the caller's identity.
pub fn take_recovery_result(identity: AgentIdentity<'_>, request_id: &str) -> std::io::Result<Option<RecoveryResult>> {
    take_at(BrowserRuntimePaths::resolve().root(), identity, request_id)
}

fn take_at(root: &Path, identity: AgentIdentity<'_>, request_id: &str) -> std::io::Result<Option<RecoveryResult>> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(None);
    }
    let result_path = path(root, request_id, "result");
    if !result_path.exists() {
        return Ok(None);
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    let Some(result) = read_json::<RecoveryResult>(&result_path)? else {
        return Ok(None);
    };
    if result.request_id != request_id
        || result.actor != identity.actor
        || Some(result.host_instance.as_str()) != identity.host_instance
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "recovery result identity mismatch",
        ));
    }
    std::fs::remove_file(result_path)?;
    Ok(Some(result))
}

fn directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-recovery")
}
fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

pub(super) fn retain_scope(
    allocation: &horizon_browser::RemoteAllocation,
    manifest: &super::BrowserManifest,
    host: &str,
) {
    let workspace = manifest
        .workspace
        .as_ref()
        .filter(|workspace| workspace.host_instance == host)
        .map(|workspace| workspace.local_id.clone());
    allocation.retain_scope(horizon_browser::RemoteAllocationScope {
        host: host.to_string(),
        workspace,
        owner: manifest.owner.as_ref().map(|owner| owner.name.clone()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_captures_the_final_owner_under_the_manifest_lock() {
        use super::super::{
            BrowserManifest, ManifestCoordination, ManifestOwner, ManifestWorkspace, manifest_path_for_root, update_at,
            write_at,
        };
        let root = tempfile::tempdir().expect("root");
        let allocation = horizon_browser::RemoteAllocation::default();
        allocation.mark_published();
        let coordinator = ManifestCoordination::with_remote_allocation(Some(allocation.clone()));
        let path = manifest_path_for_root(root.path(), "retired-panel");
        write_at(
            &path,
            &BrowserManifest {
                panel_local_id: "retired-panel".into(),
                host: Some("host-a".into()),
                owner: Some(ManifestOwner {
                    name: "owner-a".into(),
                    tty: None,
                    updated_at: 0,
                }),
                workspace: Some(ManifestWorkspace::new(
                    "host-a",
                    "workspace-a",
                    vec!["owner-a".into(), "owner-b".into()],
                )),
                ..BrowserManifest::default()
            },
        )
        .expect("manifest");
        update_at(&path, "retired-panel", |manifest| {
            manifest.owner.as_mut().expect("owner").name = "owner-b".into();
        })
        .expect("ownership changed after last host poll");
        assert_eq!(
            coordinator.remove_at(
                root.path(),
                "retired-panel",
                "host-a",
                std::time::Duration::from_secs(1)
            ),
            Some(true)
        );
        allocation.cancel_before_launch(); // complete the synthetic driver after its coordinator retired
        assert_eq!(allocation.status_for("host-a", "owner-a", "workspace-a", true), None);
        assert_eq!(
            allocation.status_for("host-a", "owner-b", "workspace-a", false),
            Some(RemoteRecoveryStatus::Released)
        );
        assert_eq!(allocation.status_for("host-b", "owner-b", "workspace-a", true), None);
        assert!(!path.exists());
    }

    #[test]
    fn host_and_actor_are_checked_without_a_live_browser_manifest() {
        let root = tempfile::tempdir().expect("root");
        let actor = AgentIdentity::new("horizon:agent-a", Some("host-a"));
        let id = enqueue_at(root.path(), actor, None).expect("enqueue");
        assert!(claim_at(root.path(), "host-b").expect("other host").is_empty());
        let requests = claim_at(root.path(), "host-a").expect("claim");
        assert_eq!(requests.len(), 1);
        assert!(claim_at(root.path(), "host-a").expect("reclaim").is_empty());
        complete_at(root.path(), &requests[0].result(Vec::new(), None)).expect("complete");
        assert!(take_at(root.path(), AgentIdentity::new("horizon:agent-b", Some("host-a")), &id).is_err());
        assert!(take_at(root.path(), AgentIdentity::new("horizon:agent-a", Some("host-b")), &id).is_err());
        assert!(take_at(root.path(), actor, &id).expect("result").is_some());
        assert!(enqueue_at(root.path(), AgentIdentity::new("external", Some("host-a")), None).is_err());
        assert!(enqueue_at(root.path(), AgentIdentity::new("horizon:agent-a", None), None).is_err());
    }
}
