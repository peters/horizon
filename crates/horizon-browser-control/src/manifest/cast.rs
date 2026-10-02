//! Workspace-scoped private coordination for the public casting tool.
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
use crate::paths::{BrowserRuntimePaths, safe_local_id};

/// Casting controls scoped to the calling agent's current workspace.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum CastOperation {
    Discover,
    Sources,
    Status,
    Paired,
    Forget {
        receiver_id: String,
    },
    Start {
        receiver_id: String,
        source: CastSource,
        #[serde(default)]
        orientation: CastOrientation,
        #[serde(default)]
        resolution: CastResolution,
    },
    Pair {
        receiver_id: String,
        pin: String,
    },
    Stop {
        receiver_id: String,
    },
}
impl std::fmt::Debug for CastOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let operation = match self {
            Self::Discover => "discover",
            Self::Sources => "sources",
            Self::Status => "status",
            Self::Paired => "paired",
            Self::Forget { .. } => "forget",
            Self::Start { .. } => "start",
            Self::Pair { .. } => "pair",
            Self::Stop { .. } => "stop",
        };
        f.debug_struct("CastOperation")
            .field("operation", &operation)
            .finish_non_exhaustive()
    }
}
impl Drop for CastOperation {
    fn drop(&mut self) {
        if let Self::Pair { pin, .. } = self {
            zeroize::Zeroize::zeroize(pin);
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CastSource {
    Panel { id: String },
    Workspace { id: String },
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CastOrientation {
    #[default]
    Landscape,
    Portrait,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum CastResolution {
    #[serde(rename = "720p")]
    Hd720,
    #[default]
    #[serde(rename = "1080p")]
    FullHd1080,
    #[serde(rename = "4k")]
    Uhd4k,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CastReceiver {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CastSourceInfo {
    pub source: CastSource,
    pub name: String,
    pub available: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CastSessionInfo {
    pub receiver_id: String,
    pub source: CastSource,
    pub orientation: CastOrientation,
    pub resolution: CastResolution,
    pub state: String,
    pub frames: u64,
    pub encoder: Option<String>,
    pub encoder_fallback: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CastOutcome {
    pub receivers: Vec<CastReceiver>,
    pub paired_receivers: Vec<CastReceiver>,
    pub sources: Vec<CastSourceInfo>,
    pub sessions: Vec<CastSessionInfo>,
    pub discovering: bool,
    pub error: Option<String>,
}
impl CastOutcome {
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            error: Some(message.into()),
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub deadline_at_millis: i64,
    pub operation: CastOperation,
}

#[derive(Deserialize, Serialize)]
struct ResultEnvelope {
    actor: String,
    host_instance: String,
    outcome: CastOutcome,
}

fn directory(root: &Path) -> PathBuf {
    root.join("runtime/cast-requests")
}
fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

/// Queue a bounded request. Only Horizon-injected identities may use this API.
/// # Errors
/// Rejects missing host identity, invalid actors, full queues and storage failures.
pub fn enqueue(identity: AgentIdentity<'_>, operation: CastOperation, timeout: Duration) -> io::Result<Request> {
    enqueue_at(BrowserRuntimePaths::resolve().root(), identity, operation, timeout)
}

/// Queue a request under an explicit runtime root.
///
/// # Errors
/// Rejects missing host identity, invalid actors, full queues and storage failures.
pub fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    operation: CastOperation,
    timeout: Duration,
) -> io::Result<Request> {
    super::agent::validate_actor(identity.actor)?;
    let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
    if !identity.workspace_scoped() || host.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cast requires an agent launched inside Horizon with its host identity",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if request_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Casting request queue is full",
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

/// Atomically claim only requests addressed to this host. The UI checks live workspace and ownership.
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block the queue.
pub fn claim(host: &str) -> io::Result<Vec<Request>> {
    claim_at(BrowserRuntimePaths::resolve().root(), host)
}

/// Same as [`claim`], against an explicit runtime root.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block the queue.
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

/// Publish a host result after checking the current board state.
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete(request: &Request, outcome: CastOutcome) -> io::Result<()> {
    complete_at(BrowserRuntimePaths::resolve().root(), request, outcome)
}

/// Same as [`complete`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete_at(root: &Path, request: &Request, outcome: CastOutcome) -> io::Result<()> {
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
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result(request: &Request) -> io::Result<Option<CastOutcome>> {
    take_result_at(BrowserRuntimePaths::resolve().root(), request)
}

/// Same as [`take_result`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result_at(root: &Path, request: &Request) -> io::Result<Option<CastOutcome>> {
    let path = path(root, &request.request_id, "result");
    let Some(result) = read_json::<ResultEnvelope>(&path)? else {
        return Ok(None);
    };
    if result.actor != request.actor || result.host_instance != request.host_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Casting result identity mismatch",
        ));
    }
    std::fs::remove_file(path)?;
    Ok(Some(result.outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolates_hosts_and_claims_a_request_once() {
        let root = tempfile::tempdir().expect("root");
        let host = uuid::Uuid::new_v4().to_string();
        let identity = AgentIdentity::new("horizon:synthetic-agent", Some(&host));
        let request =
            enqueue_at(root.path(), identity, CastOperation::Status, Duration::from_secs(5)).expect("enqueue");
        assert!(claim_at(root.path(), "other-host").expect("claim").is_empty());
        let requests = claim_at(root.path(), &host).expect("claim");
        assert_eq!(requests.len(), 1);
        assert!(claim_at(root.path(), &host).expect("second claim").is_empty());
        complete_at(root.path(), &requests[0], CastOutcome::default()).expect("complete");
        let mut outsider = request.clone();
        outsider.actor = "horizon:other-agent".into();
        assert!(take_result_at(root.path(), &outsider).is_err());
        assert!(take_result_at(root.path(), &request).expect("own result").is_some());
    }
    #[test]
    fn start_resolution_has_an_explicit_default_and_rejects_unknown_values() {
        let start =
            serde_json::json!({"operation":"start","receiver_id":"synthetic","source":{"kind":"panel","id":"source"}});
        let default: CastOperation = serde_json::from_value(start.clone()).expect("default");
        assert!(matches!(
            default,
            CastOperation::Start {
                resolution: CastResolution::FullHd1080,
                ..
            }
        ));
        for (text, expected) in [
            ("720p", CastResolution::Hd720),
            ("1080p", CastResolution::FullHd1080),
            ("4k", CastResolution::Uhd4k),
        ] {
            let mut request = start.clone();
            request["resolution"] = text.into();
            let parsed: CastOperation = serde_json::from_value(request).expect("supported resolution");
            assert!(matches!(parsed, CastOperation::Start { resolution, .. } if resolution == expected));
        }
        let mut invalid = start;
        invalid["resolution"] = "8k".into();
        assert!(serde_json::from_value::<CastOperation>(invalid).is_err());
    }
    #[test]
    fn rejects_unscoped_callers_and_hides_pin_from_debug() {
        let root = tempfile::tempdir().expect("root");
        assert!(
            enqueue_at(
                root.path(),
                AgentIdentity::new("external", None),
                CastOperation::Discover,
                Duration::from_secs(5)
            )
            .is_err()
        );
        let operation = CastOperation::Pair {
            receiver_id: "synthetic".into(),
            pin: "1234".into(),
        };
        assert!(!format!("{operation:?}").contains("1234"));
        assert!(serde_json::from_value::<CastOperation>(serde_json::json!({"operation":"start","receiver_id":"synthetic","source":{"kind":"desktop","id":"display"}})).is_err());
    }
}
