//! Private, bounded host requests for browser panel size changes.
//!
//! Mirrors the visibility queue: only the owning agent, from inside the
//! panel's workspace, on the host that launched it, may resize a panel.
//! The host applies the size through the board and stamps it back on the
//! manifest before the result is published.

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
pub enum BrowserResizeAuditStatus {
    Queued,
    Dispatched,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserResizeRequest {
    pub request_id: String,
    pub actor: String,
    /// The Horizon host that launched the requesting agent; only that host
    /// may claim the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance: Option<String>,
    pub panel_local_id: String,
    /// Target viewport width in CSS pixels; the panel keeps its current
    /// width when the request predated sized resizes.
    pub width: u32,
    /// Target viewport height in CSS pixels; the panel keeps its current
    /// height when the request predated sized resizes.
    pub height: u32,
    pub requested_at_millis: i64,
    pub deadline_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by_pid: Option<u32>,
}

impl BrowserResizeRequest {
    /// An unclaimed request for host tests; never written to the queue.
    #[doc(hidden)]
    #[must_use]
    pub fn for_tests(actor: &str, panel_local_id: &str, width: u32, height: u32, deadline_at_millis: i64) -> Self {
        Self {
            request_id: new_action_id(),
            actor: actor.to_string(),
            host_instance: None,
            panel_local_id: panel_local_id.to_string(),
            width,
            height,
            requested_at_millis: 0,
            deadline_at_millis,
            claimed_by_pid: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserResizeResult {
    pub request_id: String,
    pub actor: String,
    pub panel_local_id: String,
    pub outcome: BrowserResizeOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserResizeOutcome {
    Ready { width: u32, height: u32 },
    Failed { code: String, message: String },
}

impl BrowserResizeResult {
    /// A completed resize reporting the viewport the panel renders after the
    /// resize. That is not always the requested size: the board applies the
    /// resize within the workspace's collision scope, so a clamped panel
    /// renders a different viewport, and the result must say so.
    #[must_use]
    pub fn ready(request: &BrowserResizeRequest, width: u32, height: u32) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserResizeOutcome::Ready { width, height },
        }
    }

    #[must_use]
    pub fn failed(request: &BrowserResizeRequest, code: &str, message: &str) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            panel_local_id: request.panel_local_id.clone(),
            outcome: BrowserResizeOutcome::Failed {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

/// Queue a size change for the Horizon host containing both the agent and
/// browser panel.
///
/// # Errors
/// Returns an error when the identity is not a Horizon agent with a known
/// host instance, is outside the panel's workspace, does not own the live
/// panel, when the private queue is full, or when coordination storage
/// cannot be updated.
pub fn enqueue_resize(
    identity: AgentIdentity<'_>,
    panel_local_id: &str,
    width: u32,
    height: u32,
    timeout: Duration,
) -> std::io::Result<String> {
    enqueue_at(
        HorizonHome::resolve().root(),
        identity,
        panel_local_id,
        width,
        height,
        timeout,
    )
}

fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    panel_local_id: &str,
    width: u32,
    height: u32,
    timeout: Duration,
) -> std::io::Result<String> {
    let actor = identity.actor;
    super::agent::validate_actor(actor)?;
    if !actor_is_workspace_scoped(actor) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel size can be changed only by an agent launched inside Horizon",
        ));
    }
    let Some(host_instance) = identity.host_instance else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel size can be changed only when the launching Horizon host instance is known",
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

    let directory = resize_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    prune_at(&directory)?;
    if request_count(&directory)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "browser resize queue is full",
        ));
    }

    let request_id = new_action_id();
    let requested_at_millis = super::now_millis();
    let timeout_millis = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
    let request = BrowserResizeRequest {
        request_id: request_id.clone(),
        actor: actor.to_string(),
        host_instance: Some(host_instance.to_string()),
        panel_local_id: panel_local_id.to_string(),
        width,
        height,
        requested_at_millis,
        deadline_at_millis: requested_at_millis.saturating_add(timeout_millis),
        claimed_by_pid: None,
    };
    let path = request_path(root, &request_id);
    write_private_json(&path, &request)?;
    if let Err(error) = record_status_at(root, &request, BrowserResizeAuditStatus::Queued) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(request_id)
}

/// List unclaimed resize requests under `root`.
///
/// # Errors
/// Returns an error when the private request directory cannot be read.
pub fn list_resize_requests_in(root: &Path) -> std::io::Result<Vec<BrowserResizeRequest>> {
    let directory = resize_directory(root);
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
        let Some(request) = read_json::<BrowserResizeRequest>(&entry.path())? else {
            continue;
        };
        if safe_local_id(&request.request_id) == encoded_id && request.claimed_by_pid.is_none() {
            requests.push(request);
        }
    }
    requests.sort_by_key(|request| request.requested_at_millis);
    Ok(requests)
}

/// Claim one resize request under `root` for the exact actor when it names
/// this host.
///
/// # Errors
/// Returns an error for an identity mismatch or coordination failure.
/// `Ok(None)` means another host already claimed the request or the request
/// belongs to a different Horizon host instance.
pub fn claim_resize_request_in(
    root: &Path,
    request_id: &str,
    actor: &str,
    host_instance: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserResizeRequest>> {
    let directory = resize_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let path = request_path(root, request_id);
    let Some(mut request) = read_json::<BrowserResizeRequest>(&path)? else {
        return Ok(None);
    };
    if request.request_id != request_id || request.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser resize request identity did not match its path or actor",
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

/// Publish a resize result and remove the request.
///
/// # Errors
/// Returns an error when the private result cannot be written atomically.
pub fn complete_resize_request_in(root: &Path, result: &BrowserResizeResult) -> std::io::Result<()> {
    let directory = resize_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    write_private_json(&result_path(root, &result.request_id), result)?;
    match std::fs::remove_file(request_path(root, &result.request_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Consume a resize result for the exact requesting actor.
///
/// # Errors
/// Returns an error for invalid data, identity mismatch, or filesystem failure.
pub fn take_resize_result(request_id: &str, actor: &str) -> std::io::Result<Option<BrowserResizeResult>> {
    take_at(HorizonHome::resolve().root(), request_id, actor)
}

fn take_at(root: &Path, request_id: &str, actor: &str) -> std::io::Result<Option<BrowserResizeResult>> {
    let directory = resize_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let path = result_path(root, request_id);
    if read_json::<BrowserResizeResult>(&path)?.is_none() {
        return Ok(None);
    }
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let Some(result) = read_json::<BrowserResizeResult>(&path)? else {
        return Ok(None);
    };
    if result.request_id != request_id || result.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser resize result identity did not match its path or actor",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(result)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Append a resize lifecycle state to the panel audit journal under `root`.
///
/// # Errors
/// Returns an error for invalid identity or audit storage failure.
pub fn record_resize_status_in(
    root: &Path,
    request: &BrowserResizeRequest,
    status: BrowserResizeAuditStatus,
) -> std::io::Result<()> {
    record_status_at(root, request, status)
}

fn record_status_at(
    root: &Path,
    request: &BrowserResizeRequest,
    status: BrowserResizeAuditStatus,
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
                BrowserResizeAuditStatus::Queued => horizon_browser::BrowserAuditStatus::Queued,
                BrowserResizeAuditStatus::Dispatched => horizon_browser::BrowserAuditStatus::Dispatched,
                BrowserResizeAuditStatus::Completed => horizon_browser::BrowserAuditStatus::Completed,
                BrowserResizeAuditStatus::Failed => horizon_browser::BrowserAuditStatus::Failed,
            },
            BrowserAuditAction::Viewport {
                width: request.width,
                height: request.height,
            },
        ),
    )
}

fn resize_directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-resize")
}

fn request_path(root: &Path, request_id: &str) -> PathBuf {
    resize_directory(root).join(format!("{}.request.json", safe_local_id(request_id)))
}

fn result_path(root: &Path, request_id: &str) -> PathBuf {
    resize_directory(root).join(format!("{}.result.json", safe_local_id(request_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::{BrowserManifest, ManifestOwner, manifest_path_for_root, write_at};

    #[test]
    fn request_requires_live_ownership_and_is_audited() {
        let root = tempfile::tempdir().expect("isolated resize root");
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
                enqueue_at(
                    root.path(),
                    refused,
                    panel_local_id,
                    1920,
                    1080,
                    Duration::from_secs(30)
                )
                .expect_err("identity outside the contract must fail")
                .kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        let request_id = enqueue_at(
            root.path(),
            identity,
            panel_local_id,
            1920,
            1080,
            Duration::from_secs(30),
        )
        .expect("enqueue resize");
        assert!(
            claim_resize_request_in(root.path(), &request_id, actor, "host-b", 41)
                .expect("other host claim")
                .is_none(),
            "a host that did not launch the agent must not claim its request"
        );
        let request = claim_resize_request_in(root.path(), &request_id, actor, "host-a", 42)
            .expect("claim resize")
            .expect("request");
        assert_eq!((request.width, request.height), (1920, 1080));
        assert_eq!(request.host_instance.as_deref(), Some("host-a"));
        record_resize_status_in(root.path(), &request, BrowserResizeAuditStatus::Dispatched).expect("audit dispatch");
        record_resize_status_in(root.path(), &request, BrowserResizeAuditStatus::Completed).expect("audit completion");
        let result = BrowserResizeResult::ready(&request, 1920, 1080);
        complete_resize_request_in(root.path(), &result).expect("complete resize");
        assert_eq!(
            take_at(root.path(), &request_id, actor).expect("take resize"),
            Some(result)
        );

        let audit =
            super::super::audit::read_at(&super::super::audit::audit_path_for_root(root.path(), panel_local_id))
                .expect("read resize audit");
        assert_eq!(audit.len(), 3);
        assert!(audit.iter().all(|entry| {
            entry.action_id == request_id
                && entry.action
                    == BrowserAuditAction::Viewport {
                        width: 1920,
                        height: 1080,
                    }
        }));
    }

    #[test]
    fn claim_and_take_require_the_exact_request_identity() {
        let root = tempfile::tempdir().expect("isolated resize root");
        let actor = "horizon:agent-panel";
        let request = BrowserResizeRequest {
            request_id: "action-1".to_string(),
            actor: actor.to_string(),
            host_instance: Some("host-a".to_string()),
            panel_local_id: "browser-panel".to_string(),
            width: 375,
            height: 812,
            requested_at_millis: 0,
            deadline_at_millis: i64::MAX,
            claimed_by_pid: None,
        };
        write_private_json(&request_path(root.path(), "action-1"), &request).expect("write request");

        assert_eq!(
            claim_resize_request_in(root.path(), "action-1", "horizon:other", "host-a", 7)
                .expect_err("a different actor must not claim")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        let claimed = claim_resize_request_in(root.path(), "action-1", actor, "host-a", 7)
            .expect("claim")
            .expect("request");
        assert!(
            claim_resize_request_in(root.path(), "action-1", actor, "host-a", 8)
                .expect("second claim")
                .is_none(),
            "a claimed request is single-use"
        );

        let result = BrowserResizeResult::failed(&claimed, "request_expired", "the request expired");
        complete_resize_request_in(root.path(), &result).expect("complete failure");
        assert_eq!(
            take_at(root.path(), "action-1", "horizon:other")
                .expect_err("the wrong actor must not consume")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(
            take_at(root.path(), "action-1", actor).expect("right actor"),
            Some(result)
        );
        assert!(take_at(root.path(), "action-1", actor).expect("take once").is_none());
    }

    #[test]
    fn older_requests_and_results_keep_parsing() {
        let request: BrowserResizeRequest = serde_json::from_value(serde_json::json!({
            "request_id": "r1", "actor": "horizon:agent-panel",
            "panel_local_id": "browser-panel", "width": 800, "height": 600,
            "requested_at_millis": 0, "deadline_at_millis": 1
        }))
        .expect("request without a host instance parses");
        assert_eq!(request.host_instance, None);
        let result: BrowserResizeResult = serde_json::from_value(serde_json::json!({
            "request_id": "r1", "actor": "horizon:agent-panel",
            "panel_local_id": "browser-panel",
            "outcome": { "status": "ready", "width": 800, "height": 600 }
        }))
        .expect("ready result parses");
        assert_eq!(
            result,
            BrowserResizeResult {
                request_id: "r1".to_string(),
                actor: "horizon:agent-panel".to_string(),
                panel_local_id: "browser-panel".to_string(),
                outcome: BrowserResizeOutcome::Ready {
                    width: 800,
                    height: 600
                },
            }
        );
    }
}
