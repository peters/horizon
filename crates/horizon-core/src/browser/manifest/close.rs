//! Private, bounded host requests to close a browser panel: the session
//! stops and any remote allocation is released by the panel's own teardown.
//! Mirrors the visibility queue; only the owning agent, from inside the
//! panel's workspace, on the host that launched it, may close a panel.

use std::path::{Path, PathBuf};
use std::time::Duration;

use horizon_browser::{BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, new_action_id};
use serde::{Deserialize, Serialize};

use super::request_queue::{
    MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json,
};
use super::workspace::{AgentIdentity, OUTSIDE_WORKSPACE_MESSAGE};
use super::{ManifestLock, actor_is_workspace_scoped};
use crate::horizon_home::{HorizonHome, safe_local_id};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserCloseAuditStatus {
    Queued,
    Dispatched,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserCloseRequest {
    pub request_id: String,
    pub actor: String,
    /// The Horizon host that launched the requesting agent; only that host
    /// may claim the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance: Option<String>,
    pub panel_local_id: String,
    pub requested_at_millis: i64,
    pub deadline_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by_pid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserCloseResult {
    pub request_id: String,
    pub actor: String,
    pub panel_local_id: String,
    pub outcome: BrowserCloseOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserCloseOutcome {
    /// The panel is gone from the host; its session teardown has begun.
    Closed,
    Failed {
        code: String,
        message: String,
    },
}

impl BrowserCloseResult {
    #[must_use]
    pub fn closed(request: &BrowserCloseRequest) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserCloseOutcome::Closed,
        }
    }

    #[must_use]
    pub fn failed(request: &BrowserCloseRequest, code: &str, message: &str) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserCloseOutcome::Failed {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

/// Queue a close for the Horizon host containing both the agent and the
/// browser panel.
///
/// # Errors
/// Returns an error when the identity is not a Horizon agent with a known
/// host instance, is outside the panel's workspace, does not own the live
/// panel, when the private queue is full, or when coordination storage
/// cannot be updated.
pub fn enqueue_close(identity: AgentIdentity<'_>, panel_local_id: &str, timeout: Duration) -> std::io::Result<String> {
    enqueue_at(HorizonHome::resolve().root(), identity, panel_local_id, timeout)
}

fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    panel_local_id: &str,
    timeout: Duration,
) -> std::io::Result<String> {
    let actor = identity.actor;
    super::agent::validate_actor(actor)?;
    if !actor_is_workspace_scoped(actor) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panels can be closed only by an agent launched inside Horizon",
        ));
    }
    let Some(host_instance) = identity.host_instance else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panels can be closed only when the launching Horizon host instance is known",
        ));
    };
    let manifest_path = super::manifest_path_for_root(root, panel_local_id);
    let manifest = super::read_at(&manifest_path)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser panel is not live"))?;
    if !manifest.permits(identity) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OUTSIDE_WORKSPACE_MESSAGE,
        ));
    }
    if manifest
        .live_owner(super::now_millis())
        .map(|owner| owner.name.as_str())
        != Some(actor)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel is not owned by the requesting agent",
        ));
    }

    let directory = close_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    prune_at(&directory)?;
    if request_count(&directory)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "browser close queue is full",
        ));
    }

    let request_id = new_action_id();
    let requested_at_millis = super::now_millis();
    let timeout_millis = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
    let request = BrowserCloseRequest {
        request_id: request_id.clone(),
        actor: actor.to_string(),
        host_instance: Some(host_instance.to_string()),
        panel_local_id: panel_local_id.to_string(),
        requested_at_millis,
        deadline_at_millis: requested_at_millis.saturating_add(timeout_millis),
        claimed_by_pid: None,
    };
    let path = request_path(root, &request_id);
    write_private_json(&path, &request)?;
    if let Err(error) = record_status_at(root, &request, BrowserCloseAuditStatus::Queued) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(request_id)
}

/// List unclaimed close requests.
///
/// # Errors
/// Returns an error when the private request directory cannot be read.
pub fn list_close_requests() -> std::io::Result<Vec<BrowserCloseRequest>> {
    list_at(HorizonHome::resolve().root())
}

fn list_at(root: &Path) -> std::io::Result<Vec<BrowserCloseRequest>> {
    let directory = close_directory(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let mut requests = Vec::new();
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        let Some(encoded_id) = file_name.strip_suffix(".request.json") else {
            continue;
        };
        let Some(request) = read_json::<BrowserCloseRequest>(&entry.path())? else {
            continue;
        };
        if safe_local_id(&request.request_id) == encoded_id && request.claimed_by_pid.is_none() {
            requests.push(request);
        }
    }
    requests.sort_by_key(|request| request.requested_at_millis);
    Ok(requests)
}

/// Claim one close request for the exact actor when it names this host.
///
/// # Errors
/// Returns an error for an identity mismatch or coordination failure.
/// `Ok(None)` means another host already claimed the request or the request
/// belongs to a different Horizon host instance.
pub fn claim_close_request(
    request_id: &str,
    actor: &str,
    host_instance: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserCloseRequest>> {
    claim_at(
        HorizonHome::resolve().root(),
        request_id,
        actor,
        host_instance,
        claimant_pid,
    )
}

fn claim_at(
    root: &Path,
    request_id: &str,
    actor: &str,
    host_instance: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserCloseRequest>> {
    let directory = close_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let path = request_path(root, request_id);
    let Some(mut request) = read_json::<BrowserCloseRequest>(&path)? else {
        return Ok(None);
    };
    if request.request_id != request_id || request.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser close request identity did not match its path or actor",
        ));
    }
    // Compared under the queue lock: a second live host running a copy of
    // the same session shares the actor but never this host instance.
    if request.claimed_by_pid.is_some() || request.host_instance.as_deref() != Some(host_instance) {
        return Ok(None);
    }
    request.claimed_by_pid = Some(claimant_pid);
    write_private_json(&path, &request)?;
    Ok(Some(request))
}

/// Publish a close result and remove the request.
///
/// # Errors
/// Returns an error when the private result cannot be written atomically.
pub fn complete_close_request(result: &BrowserCloseResult) -> std::io::Result<()> {
    complete_at(HorizonHome::resolve().root(), result)
}

fn complete_at(root: &Path, result: &BrowserCloseResult) -> std::io::Result<()> {
    let directory = close_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    write_private_json(&result_path(root, &result.request_id), result)?;
    match std::fs::remove_file(request_path(root, &result.request_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Consume a close result for the exact requesting actor.
///
/// # Errors
/// Returns an error for invalid data, identity mismatch, or filesystem failure.
pub fn take_close_result(request_id: &str, actor: &str) -> std::io::Result<Option<BrowserCloseResult>> {
    take_at(HorizonHome::resolve().root(), request_id, actor)
}

fn take_at(root: &Path, request_id: &str, actor: &str) -> std::io::Result<Option<BrowserCloseResult>> {
    let directory = close_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let path = result_path(root, request_id);
    if read_json::<BrowserCloseResult>(&path)?.is_none() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let Some(result) = read_json::<BrowserCloseResult>(&path)? else {
        return Ok(None);
    };
    if result.request_id != request_id || result.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser close result identity did not match its path or actor",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(result)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Append a close lifecycle state to the panel audit journal.
///
/// # Errors
/// Returns an error for invalid identity or audit storage failure.
pub fn record_close_status(request: &BrowserCloseRequest, status: BrowserCloseAuditStatus) -> std::io::Result<()> {
    record_status_at(HorizonHome::resolve().root(), request, status)
}

fn record_status_at(
    root: &Path,
    request: &BrowserCloseRequest,
    status: BrowserCloseAuditStatus,
) -> std::io::Result<()> {
    super::agent::validate_actor(&request.actor)?;
    super::audit::append_at_path(
        &super::audit::audit_path_for_root(root, &request.panel_local_id),
        &BrowserAuditEntry::new(
            request.request_id.clone(),
            BrowserAuditActor::Agent {
                name: request.actor.clone(),
            },
            match status {
                BrowserCloseAuditStatus::Queued => horizon_browser::BrowserAuditStatus::Queued,
                BrowserCloseAuditStatus::Dispatched => horizon_browser::BrowserAuditStatus::Dispatched,
                BrowserCloseAuditStatus::Completed => horizon_browser::BrowserAuditStatus::Completed,
                BrowserCloseAuditStatus::Failed => horizon_browser::BrowserAuditStatus::Failed,
            },
            BrowserAuditAction::PanelClose,
        ),
    )
}

fn close_directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-close")
}

fn request_path(root: &Path, request_id: &str) -> PathBuf {
    close_directory(root).join(format!("{}.request.json", safe_local_id(request_id)))
}

fn result_path(root: &Path, request_id: &str) -> PathBuf {
    close_directory(root).join(format!("{}.result.json", safe_local_id(request_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::{BrowserManifest, ManifestOwner, manifest_path_for_root, write_at};

    #[test]
    fn request_requires_live_ownership_and_is_audited() {
        let root = tempfile::tempdir().expect("isolated close root");
        let actor = "horizon:agent-panel";
        let identity = AgentIdentity::new(actor, Some("host-a"));
        let panel_local_id = "browser-panel";
        write_at(
            &manifest_path_for_root(root.path(), panel_local_id),
            &BrowserManifest {
                panel_local_id: panel_local_id.to_string(),
                host: Some("host-a".to_string()),
                workspace: Some(super::super::ManifestWorkspace::new(
                    "host-a",
                    "ws-a",
                    vec![actor.to_string()],
                )),
                owner: Some(ManifestOwner {
                    name: actor.to_string(),
                    tty: None,
                    updated_at: super::super::now_millis(),
                }),
                ..BrowserManifest::default()
            },
        )
        .expect("write manifest");

        for refused in [
            AgentIdentity::new("external", Some("host-a")),
            AgentIdentity::new("horizon:other-agent", Some("host-a")),
            AgentIdentity::new(actor, Some("host-b")),
            AgentIdentity::new(actor, None),
        ] {
            assert_eq!(
                enqueue_at(root.path(), refused, panel_local_id, Duration::from_secs(30))
                    .expect_err("identity outside the contract must fail")
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        let request_id =
            enqueue_at(root.path(), identity, panel_local_id, Duration::from_secs(30)).expect("enqueue close");
        assert!(
            claim_at(root.path(), &request_id, actor, "host-b", 41)
                .expect("other host claim")
                .is_none(),
            "a host that did not launch the agent must not claim its request"
        );
        let request = claim_at(root.path(), &request_id, actor, "host-a", 42)
            .expect("claim close")
            .expect("request");
        assert_eq!(request.host_instance.as_deref(), Some("host-a"));
        record_status_at(root.path(), &request, BrowserCloseAuditStatus::Dispatched).expect("audit dispatch");
        record_status_at(root.path(), &request, BrowserCloseAuditStatus::Completed).expect("audit completion");
        let result = BrowserCloseResult::closed(&request);
        complete_at(root.path(), &result).expect("complete close");
        assert_eq!(
            take_at(root.path(), &request_id, actor).expect("take close"),
            Some(result)
        );

        let audit =
            super::super::audit::read_at(&super::super::audit::audit_path_for_root(root.path(), panel_local_id))
                .expect("read close audit");
        assert_eq!(audit.len(), 3);
        assert!(
            audit
                .iter()
                .all(|entry| { entry.action_id == request_id && entry.action == BrowserAuditAction::PanelClose })
        );
    }
}
