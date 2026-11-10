//! Private host coordination for the public `block_panel` tool.
//!
//! An agent asks Horizon to draw a card in its own workspace: `create` with a
//! document, `update` with a new one, and `list`, `inspect`, `visibility` and
//! `close` around it. A request travels the path of a Device panel request: a
//! bounded file under `runtime/block-panel-requests`, claimed by this host's
//! UI on the coordination worker, applied on the UI thread, and answered with
//! one result file that only the asking actor and its host consume.
//!
//! The document is untrusted text from an agent that Horizon draws in a
//! person's workspace, so the UI validates every document again before it
//! reaches a panel, and the whole request is bounded in size here.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AgentIdentity, ManifestLock, now_millis,
    request_queue::{MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json},
};
use crate::{
    block::BlockDocument,
    paths::{BrowserRuntimePaths, safe_local_id},
};

/// Most cards one agent may own in one workspace at a time.
pub const MAX_CARDS_PER_OWNER: usize = 8;
/// Largest serialized request accepted. A document is smaller than this, so a
/// queue of them stays bounded whatever a caller sends.
const MAX_REQUEST_BYTES: usize = 96 * 1024;

/// Card commands. The document belongs to the agent; the panel id, revision
/// and ownership belong to the host.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum Operation {
    /// Draw a new card in the calling agent's workspace. Does not take focus.
    Create {
        /// One line, at most 80 characters; the host trims it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The document to draw: `{"components": [...]}`.
        #[schemars(with = "BlockDocument")]
        document: serde_json::Value,
    },
    /// Replace the whole document of a card the caller owns.
    Update {
        panel_id: String,
        /// The document to draw: `{"components": [...]}`.
        #[schemars(with = "BlockDocument")]
        document: serde_json::Value,
    },
    /// Cards in the calling agent's workspace.
    List,
    /// One card of the calling agent's workspace, with its document.
    Inspect { panel_id: String },
    /// Show or hide a card the caller owns.
    Visibility { panel_id: String, visible: bool },
    /// Close a card the caller owns. The document is dropped with it.
    Close { panel_id: String },
}

/// One card as `list` reports it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct CardSummary {
    pub panel_id: String,
    pub title: String,
    pub revision: u64,
    /// Whether the calling agent created this card.
    pub owned_by_caller: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]
pub enum Outcome {
    Created {
        panel_id: String,
        revision: u64,
    },
    Updated {
        panel_id: String,
        revision: u64,
    },
    Panels {
        panels: Vec<CardSummary>,
    },
    Inspected {
        panel_id: String,
        title: String,
        revision: u64,
        document: BlockDocument,
        visible: bool,
        /// Whether an agent panel of the owner is still in this workspace.
        agent_present: bool,
        owned_by_caller: bool,
    },
    Visibility {
        panel_id: String,
        visible: bool,
    },
    Closed {
        panel_id: String,
    },
    Failed {
        code: String,
        message: String,
    },
}

impl Outcome {
    #[must_use]
    pub fn failed(code: &str, message: &str) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub deadline_at_millis: i64,
    pub operation: Operation,
}

#[derive(Deserialize, Serialize)]
struct ResultEnvelope {
    actor: String,
    host_instance: String,
    outcome: Outcome,
}

fn directory(root: &Path) -> PathBuf {
    root.join("runtime/block-panel-requests")
}

fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

/// Queue a bounded request. Only Horizon-injected identities may use this API.
///
/// # Errors
/// Rejects a missing host identity, an invalid actor, an oversized request, a
/// full queue and storage failures.
pub fn enqueue(identity: AgentIdentity<'_>, operation: Operation, timeout: Duration) -> io::Result<Request> {
    enqueue_at(BrowserRuntimePaths::resolve().root(), identity, operation, timeout)
}

/// Queue a request under an explicit runtime root.
///
/// # Errors
/// Rejects a missing host identity, an invalid actor, an oversized request, a
/// full queue and storage failures.
pub fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    operation: Operation,
    timeout: Duration,
) -> io::Result<Request> {
    super::agent::validate_actor(identity.actor)?;
    let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
    if !identity.workspace_scoped() || host.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "block_panel requires an agent launched inside Horizon with its host identity",
        ));
    }
    let size = serde_json::to_vec(&operation).map_err(io::Error::other)?.len();
    if size > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Block panel request is too large",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if request_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Block panel request queue is full",
        ));
    }
    let request = Request {
        request_id: horizon_browser::new_action_id(),
        actor: identity.actor.into(),
        host_instance: host.unwrap_or_default().into(),
        deadline_at_millis: now_millis().saturating_add(i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX)),
        operation,
    };
    write_private_json(&path(root, &request.request_id, "request"), &request)?;
    Ok(request)
}

/// Whether this host has a request waiting. Does not claim or remove it.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests are ignored.
pub fn has_pending(host: &str) -> io::Result<bool> {
    has_pending_at(BrowserRuntimePaths::resolve().root(), host)
}

/// Same as [`has_pending`], against an explicit runtime root.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests are ignored.
pub fn has_pending_at(root: &Path, host: &str) -> io::Result<bool> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(false);
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let Ok(Some(request)) = read_json::<Request>(&entry.path()) else {
            continue;
        };
        if request.host_instance == host && entry.path() == path(root, &request.request_id, "request") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Atomically claim only the requests addressed to this host. The UI checks
/// the live workspace, the owner and the document.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block
/// the queue.
pub fn claim(host: &str) -> io::Result<Vec<Request>> {
    claim_at(BrowserRuntimePaths::resolve().root(), host)
}

/// Same as [`claim`], against an explicit runtime root.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block
/// the queue.
pub fn claim_at(root: &Path, host: &str) -> io::Result<Vec<Request>> {
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
        let Ok(Some(request)) = read_json::<Request>(&entry.path()) else {
            continue;
        };
        if request.host_instance != host || entry.path() != path(root, &request.request_id, "request") {
            continue;
        }
        std::fs::remove_file(entry.path())?;
        requests.push(request);
    }
    Ok(requests)
}

/// Publish a host result for one claimed request.
///
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete(request: &Request, outcome: Outcome) -> io::Result<()> {
    complete_at(BrowserRuntimePaths::resolve().root(), request, outcome)
}

/// Same as [`complete`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete_at(root: &Path, request: &Request, outcome: Outcome) -> io::Result<()> {
    write_private_json(
        &path(root, &request.request_id, "result"),
        &ResultEnvelope {
            actor: request.actor.clone(),
            host_instance: request.host_instance.clone(),
            outcome,
        },
    )
}

/// Consume only a result for the exact requesting actor and host.
///
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result(request: &Request) -> io::Result<Option<Outcome>> {
    take_result_at(BrowserRuntimePaths::resolve().root(), request)
}

/// Same as [`take_result`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result_at(root: &Path, request: &Request) -> io::Result<Option<Outcome>> {
    let path = path(root, &request.request_id, "result");
    let Some(result) = read_json::<ResultEnvelope>(&path)? else {
        return Ok(None);
    };
    if result.actor != request.actor || result.host_instance != request.host_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Block panel result identity mismatch",
        ));
    }
    std::fs::remove_file(path)?;
    Ok(Some(result.outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(actor: &str) -> AgentIdentity<'_> {
        AgentIdentity::new(actor, Some(super::super::host_instance()))
    }

    fn document() -> serde_json::Value {
        serde_json::json!({
            "components": [
                {"type": "heading", "text": "cloud-1"},
                {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "warn"}]}
            ]
        })
    }

    #[test]
    fn every_operation_parses_and_an_unknown_field_is_rejected() {
        let create = serde_json::json!({"operation": "create", "title": "cloud-1", "document": document()});
        let Operation::Create { title, document: sent } = serde_json::from_value(create).expect("create") else {
            panic!("create did not parse as Create");
        };
        assert_eq!(title.as_deref(), Some("cloud-1"));
        assert_eq!(sent, document());
        for (operation, expected) in [
            (serde_json::json!({"operation": "list"}), "list"),
            (
                serde_json::json!({"operation": "update", "panel_id": "p", "document": document()}),
                "update",
            ),
            (serde_json::json!({"operation": "inspect", "panel_id": "p"}), "inspect"),
            (
                serde_json::json!({"operation": "visibility", "panel_id": "p", "visible": false}),
                "visibility",
            ),
            (serde_json::json!({"operation": "close", "panel_id": "p"}), "close"),
        ] {
            let parsed: Operation = serde_json::from_value(operation).expect("operation");
            let name = match parsed {
                Operation::Create { .. } => "create",
                Operation::Update { .. } => "update",
                Operation::List => "list",
                Operation::Inspect { .. } => "inspect",
                Operation::Visibility { .. } => "visibility",
                Operation::Close { .. } => "close",
            };
            assert_eq!(name, expected);
        }
        // Serde cannot reject an unknown field beside a unit variant, so the
        // check is asserted where a caller can send one: on a variant that
        // carries fields.
        for unknown_field in [
            serde_json::json!({"operation": "create", "document": document(), "visible": true}),
            serde_json::json!({"operation": "update", "panel_id": "p", "document": document(), "visible": true}),
        ] {
            assert!(
                serde_json::from_value::<Operation>(unknown_field.clone()).is_err(),
                "{unknown_field} must be refused"
            );
        }
        let unknown_operation = serde_json::json!({"operation": "next_click", "panel_id": "p"});
        assert!(serde_json::from_value::<Operation>(unknown_operation).is_err());
    }

    #[test]
    fn a_queued_request_round_trips_and_only_its_actor_consumes_the_result() {
        let root = tempfile::tempdir().expect("runtime root");
        let request = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::Update {
                panel_id: "card".into(),
                document: document(),
            },
            Duration::from_secs(10),
        )
        .expect("enqueue");
        assert!(has_pending_at(root.path(), super::super::host_instance()).expect("pending"));
        // Another host neither sees nor claims this request.
        assert!(!has_pending_at(root.path(), "another-host").expect("pending"));
        assert!(claim_at(root.path(), "another-host").expect("claim").is_empty());
        let claimed = claim_at(root.path(), super::super::host_instance()).expect("claim");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].operation, request.operation);
        assert!(!has_pending_at(root.path(), super::super::host_instance()).expect("pending"));

        complete_at(
            root.path(),
            &request,
            Outcome::Updated {
                panel_id: "card".into(),
                revision: 3,
            },
        )
        .expect("complete");
        let mut other = request.clone();
        other.actor = "horizon:agent-b".into();
        assert!(take_result_at(root.path(), &other).is_err(), "another actor is refused");
        let outcome = take_result_at(root.path(), &request).expect("result").expect("answer");
        assert!(matches!(outcome, Outcome::Updated { revision: 3, .. }));
        assert!(
            take_result_at(root.path(), &request).expect("result").is_none(),
            "a result is consumed once"
        );
    }

    #[test]
    fn a_request_carries_its_deadline_and_an_unsupported_identity_is_refused() {
        let root = tempfile::tempdir().expect("runtime root");
        let before = now_millis();
        let request = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::List,
            Duration::from_secs(10),
        )
        .expect("enqueue");
        assert!(request.deadline_at_millis >= before + 9_900);
        assert!(request.deadline_at_millis <= before + 10_100);
        let expired = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::List,
            Duration::from_secs(0),
        )
        .expect("enqueue");
        assert!(expired.deadline_at_millis <= now_millis());

        for (actor, host) in [("plain-actor", Some("host")), ("horizon:agent-a", None)] {
            let refused = enqueue_at(
                root.path(),
                AgentIdentity::new(actor, host),
                Operation::List,
                Duration::from_secs(5),
            );
            assert!(refused.is_err(), "{actor} with host {host:?} must be refused");
        }
    }

    #[test]
    fn an_oversized_request_is_refused_before_it_reaches_the_queue() {
        let root = tempfile::tempdir().expect("runtime root");
        let huge = serde_json::json!({"components": [{"type": "text", "text": "x".repeat(MAX_REQUEST_BYTES)}]});
        let refused = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::Update {
                panel_id: "card".into(),
                document: huge,
            },
            Duration::from_secs(5),
        );
        assert!(refused.is_err());
        assert!(!has_pending_at(root.path(), super::super::host_instance()).expect("pending"));
    }
}
