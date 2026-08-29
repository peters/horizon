//! Private, bounded host requests for browser panel visibility changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use horizon_browser::{BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, new_action_id};
use serde::{Deserialize, Serialize};

use super::ManifestLock;
use super::request_queue::{
    MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json,
};
use crate::horizon_home::{HorizonHome, safe_local_id};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserVisibilityAuditStatus {
    Queued,
    Dispatched,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserVisibilityRequest {
    pub request_id: String,
    pub actor: String,
    pub panel_local_id: String,
    pub visible: bool,
    pub requested_at_millis: i64,
    pub deadline_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by_pid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserVisibilityResult {
    pub request_id: String,
    pub actor: String,
    pub panel_local_id: String,
    pub outcome: BrowserVisibilityOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserVisibilityOutcome {
    Ready { visible: bool },
    Failed { code: String, message: String },
}

impl BrowserVisibilityResult {
    #[must_use]
    pub fn ready(request: &BrowserVisibilityRequest) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserVisibilityOutcome::Ready {
                visible: request.visible,
            },
        }
    }

    #[must_use]
    pub fn failed(request: &BrowserVisibilityRequest, code: &str, message: &str) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserVisibilityOutcome::Failed {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

/// Queue a visibility change for the Horizon host containing both the agent
/// and browser panel.
///
/// # Errors
/// Returns an error when the actor does not own the live panel, the private
/// queue is full, or coordination storage cannot be updated.
pub fn enqueue_visibility(
    actor: &str,
    panel_local_id: &str,
    visible: bool,
    timeout: Duration,
) -> std::io::Result<String> {
    enqueue_at(HorizonHome::resolve().root(), actor, panel_local_id, visible, timeout)
}

fn enqueue_at(
    root: &Path,
    actor: &str,
    panel_local_id: &str,
    visible: bool,
    timeout: Duration,
) -> std::io::Result<String> {
    super::agent::validate_actor(actor)?;
    if actor.strip_prefix("horizon:").is_none_or(str::is_empty) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel visibility can be changed only by an agent launched inside Horizon",
        ));
    }
    let manifest_path = super::manifest_path_for_root(root, panel_local_id);
    let manifest = super::read_at(&manifest_path)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser panel is not live"))?;
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

    let directory = visibility_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    prune_at(&directory)?;
    if request_count(&directory)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "browser visibility queue is full",
        ));
    }

    let request_id = new_action_id();
    let requested_at_millis = super::now_millis();
    let timeout_millis = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
    let request = BrowserVisibilityRequest {
        request_id: request_id.clone(),
        actor: actor.to_string(),
        panel_local_id: panel_local_id.to_string(),
        visible,
        requested_at_millis,
        deadline_at_millis: requested_at_millis.saturating_add(timeout_millis),
        claimed_by_pid: None,
    };
    let path = request_path(root, &request_id);
    write_private_json(&path, &request)?;
    if let Err(error) = record_status_at(root, &request, BrowserVisibilityAuditStatus::Queued) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(request_id)
}

/// List unclaimed visibility requests.
///
/// # Errors
/// Returns an error when the private request directory cannot be read.
pub fn list_visibility_requests() -> std::io::Result<Vec<BrowserVisibilityRequest>> {
    list_at(HorizonHome::resolve().root())
}

fn list_at(root: &Path) -> std::io::Result<Vec<BrowserVisibilityRequest>> {
    let directory = visibility_directory(root);
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
        let Some(request) = read_json::<BrowserVisibilityRequest>(&entry.path())? else {
            continue;
        };
        if safe_local_id(&request.request_id) == encoded_id && request.claimed_by_pid.is_none() {
            requests.push(request);
        }
    }
    requests.sort_by_key(|request| request.requested_at_millis);
    Ok(requests)
}

/// Claim one visibility request for the exact actor.
///
/// # Errors
/// Returns an error for an identity mismatch or coordination failure.
pub fn claim_visibility_request(
    request_id: &str,
    actor: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserVisibilityRequest>> {
    claim_at(HorizonHome::resolve().root(), request_id, actor, claimant_pid)
}

fn claim_at(
    root: &Path,
    request_id: &str,
    actor: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserVisibilityRequest>> {
    let directory = visibility_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let path = request_path(root, request_id);
    let Some(mut request) = read_json::<BrowserVisibilityRequest>(&path)? else {
        return Ok(None);
    };
    if request.request_id != request_id || request.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser visibility request identity did not match its path or actor",
        ));
    }
    if request.claimed_by_pid.is_some() {
        return Ok(None);
    }
    request.claimed_by_pid = Some(claimant_pid);
    write_private_json(&path, &request)?;
    Ok(Some(request))
}

/// Publish a visibility result and remove the request.
///
/// # Errors
/// Returns an error when the private result cannot be written atomically.
pub fn complete_visibility_request(result: &BrowserVisibilityResult) -> std::io::Result<()> {
    complete_at(HorizonHome::resolve().root(), result)
}

fn complete_at(root: &Path, result: &BrowserVisibilityResult) -> std::io::Result<()> {
    let directory = visibility_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    write_private_json(&result_path(root, &result.request_id), result)?;
    match std::fs::remove_file(request_path(root, &result.request_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Consume a visibility result for the exact requesting actor.
///
/// # Errors
/// Returns an error for invalid data, identity mismatch, or filesystem failure.
pub fn take_visibility_result(request_id: &str, actor: &str) -> std::io::Result<Option<BrowserVisibilityResult>> {
    take_at(HorizonHome::resolve().root(), request_id, actor)
}

fn take_at(root: &Path, request_id: &str, actor: &str) -> std::io::Result<Option<BrowserVisibilityResult>> {
    let directory = visibility_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let path = result_path(root, request_id);
    if read_json::<BrowserVisibilityResult>(&path)?.is_none() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let Some(result) = read_json::<BrowserVisibilityResult>(&path)? else {
        return Ok(None);
    };
    if result.request_id != request_id || result.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser visibility result identity did not match its path or actor",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(result)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Append a visibility lifecycle state to the panel audit journal.
///
/// # Errors
/// Returns an error for invalid identity or audit storage failure.
pub fn record_visibility_status(
    request: &BrowserVisibilityRequest,
    status: BrowserVisibilityAuditStatus,
) -> std::io::Result<()> {
    record_status_at(HorizonHome::resolve().root(), request, status)
}

fn record_status_at(
    root: &Path,
    request: &BrowserVisibilityRequest,
    status: BrowserVisibilityAuditStatus,
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
                BrowserVisibilityAuditStatus::Queued => horizon_browser::BrowserAuditStatus::Queued,
                BrowserVisibilityAuditStatus::Dispatched => horizon_browser::BrowserAuditStatus::Dispatched,
                BrowserVisibilityAuditStatus::Completed => horizon_browser::BrowserAuditStatus::Completed,
                BrowserVisibilityAuditStatus::Failed => horizon_browser::BrowserAuditStatus::Failed,
            },
            BrowserAuditAction::PanelVisibility {
                visible: request.visible,
            },
        ),
    )
}

fn visibility_directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-visibility")
}

fn request_path(root: &Path, request_id: &str) -> PathBuf {
    visibility_directory(root).join(format!("{}.request.json", safe_local_id(request_id)))
}

fn result_path(root: &Path, request_id: &str) -> PathBuf {
    visibility_directory(root).join(format!("{}.result.json", safe_local_id(request_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::{BrowserManifest, ManifestOwner, manifest_path_for_root, write_at};

    #[test]
    fn request_requires_live_ownership_and_is_audited() {
        let root = tempfile::tempdir().expect("isolated visibility root");
        let actor = "horizon:agent-panel";
        let panel_local_id = "browser-panel";
        write_at(
            &manifest_path_for_root(root.path(), panel_local_id),
            &BrowserManifest {
                panel_local_id: panel_local_id.to_string(),
                owner: Some(ManifestOwner {
                    name: actor.to_string(),
                    tty: None,
                    updated_at: super::super::now_millis(),
                }),
                ..BrowserManifest::default()
            },
        )
        .expect("write manifest");

        assert_eq!(
            enqueue_at(root.path(), "external", panel_local_id, false, Duration::from_secs(30))
                .expect_err("external actor must fail")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            enqueue_at(
                root.path(),
                "horizon:other-agent",
                panel_local_id,
                false,
                Duration::from_secs(30),
            )
            .expect_err("non-owner must fail")
            .kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let request_id =
            enqueue_at(root.path(), actor, panel_local_id, false, Duration::from_secs(30)).expect("enqueue visibility");
        let request = claim_at(root.path(), &request_id, actor, 42)
            .expect("claim visibility")
            .expect("request");
        assert!(!request.visible);
        record_status_at(root.path(), &request, BrowserVisibilityAuditStatus::Dispatched).expect("audit dispatch");
        record_status_at(root.path(), &request, BrowserVisibilityAuditStatus::Completed).expect("audit completion");
        let result = BrowserVisibilityResult::ready(&request);
        complete_at(root.path(), &result).expect("complete visibility");
        assert_eq!(
            take_at(root.path(), &request_id, actor).expect("take visibility"),
            Some(result)
        );

        let audit =
            super::super::audit::read_at(&super::super::audit::audit_path_for_root(root.path(), panel_local_id))
                .expect("read visibility audit");
        assert_eq!(audit.len(), 3);
        assert!(audit.iter().all(|entry| {
            entry.action_id == request_id && entry.action == BrowserAuditAction::PanelVisibility { visible: false }
        }));
    }
}
