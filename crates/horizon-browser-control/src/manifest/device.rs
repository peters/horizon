//! Private host coordination for the public native Device panel lifecycle.
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

/// Native viewer lifecycle only. Input belongs to the standalone device API.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum Operation {
    /// Create in the calling agent's workspace. Returns immediately; inspect for image readiness.
    Create {
        endpoint: String,
        /// Optional labels supplied by the session creator, not verified by VNC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<DeviceIdentity>,
    },
    List,
    Inspect {
        panel_id: String,
    },
    Visibility {
        panel_id: String,
        visible: bool,
    },
    /// Explicitly reconnect, acquiring an unowned (including restored) viewer.
    Reconnect {
        panel_id: String,
    },
    Close {
        panel_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Stopped,
    Connecting,
    Connected,
    Disconnected,
}

/// Machine details supplied by the session creator; never inferred from loopback.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceIdentity {
    pub machine_name: Option<String>,
    pub hostname: Option<String>,
    pub ip_addresses: Vec<std::net::IpAddr>,
    pub tailscale_name: Option<String>,
}

/// Details observed on this VNC connection, not persisted machine identity.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default)]
pub struct DeviceServerDetails {
    pub name: Option<String>,
    pub desktop_size: Option<[usize; 2]>,
}

/// Host observation, separate from request dispatch or VNC handshake success.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PanelState {
    pub panel_id: String,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<DeviceIdentity>,
    #[serde(default)]
    pub server: DeviceServerDetails,
    pub visible: bool,
    pub owned_by_caller: bool,
    pub connection: Connection,
    pub connection_error: Option<String>,
    #[serde(flatten)]
    pub image: ImageEvidence,
}

/// Reception, upload and completed-frame presentation evidence for one connection.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ImageEvidence {
    /// A decoded image was uploaded in this connection; may be stale after disconnect.
    pub image_received: bool,
    /// The most recent completed UI frame painted the connected image.
    pub image_displayed: bool,
    pub frame_sequence: u64,
    /// Worker-published image updates, including while hidden; not a heartbeat.
    #[serde(default)]
    pub received_frame_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]
pub enum Outcome {
    Panels { panels: Vec<PanelState> },
    Closed { panel_id: String },
    Failed { code: String, message: String },
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
    root.join("runtime/device-panel-requests")
}
fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

/// Queue a bounded request. Only Horizon-injected identities may use this API.
/// # Errors
/// Rejects missing host identity, invalid actors, full queues and storage failures.
pub fn enqueue(identity: AgentIdentity<'_>, operation: Operation, timeout: Duration) -> io::Result<Request> {
    enqueue_at(BrowserRuntimePaths::resolve().root(), identity, operation, timeout)
}

fn enqueue_at(
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
            "device_panel requires an agent launched inside Horizon with its host identity",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if request_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Device panel request queue is full",
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

/// Atomically claim only requests addressed to this host. The UI checks live workspace and ownership.
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block the queue.
pub fn claim(host: &str) -> io::Result<Vec<Request>> {
    claim_at(BrowserRuntimePaths::resolve().root(), host)
}

fn claim_at(root: &Path, host: &str) -> io::Result<Vec<Request>> {
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
pub fn complete(request: &Request, outcome: Outcome) -> io::Result<()> {
    complete_at(BrowserRuntimePaths::resolve().root(), request, outcome)
}

fn complete_at(root: &Path, request: &Request, outcome: Outcome) -> io::Result<()> {
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
pub fn take_result(request: &Request) -> io::Result<Option<Outcome>> {
    take_result_at(BrowserRuntimePaths::resolve().root(), request)
}

fn take_result_at(root: &Path, request: &Request) -> io::Result<Option<Outcome>> {
    let path = path(root, &request.request_id, "result");
    let Some(result) = read_json::<ResultEnvelope>(&path)? else {
        return Ok(None);
    };
    if result.actor != request.actor || result.host_instance != request.host_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Device panel result identity mismatch",
        ));
    }
    std::fs::remove_file(path)?;
    Ok(Some(result.outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reception_evidence_is_additive_and_round_trips_without_display() {
        let legacy = r#"{"panel_id":"viewer","endpoint":"127.0.0.1:5900","visible":false,"owned_by_caller":true,"connection":"connected","connection_error":null,"image_received":false,"image_displayed":false,"frame_sequence":0}"#;
        let mut panel: PanelState = serde_json::from_str(legacy).unwrap();
        assert_eq!(panel.image.received_frame_sequence, 0);
        panel.image.received_frame_sequence = 7;
        let encoded = serde_json::to_value(panel).unwrap();
        assert_eq!(encoded["received_frame_sequence"], 7);
        let decoded: PanelState = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.image.received_frame_sequence, 7);
        assert_eq!(decoded.image.frame_sequence, 0);
        assert!(!decoded.image.image_received && !decoded.image.image_displayed);
    }

    #[test]
    fn queue_is_host_bound_single_claim_and_result_is_identity_bound() {
        let root = tempfile::tempdir().unwrap();
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        let request = enqueue_at(root.path(), identity, Operation::List, Duration::from_secs(5)).unwrap();
        assert!(claim_at(root.path(), "host-b").unwrap().is_empty());
        assert_eq!(claim_at(root.path(), "host-a").unwrap().len(), 1);
        assert!(claim_at(root.path(), "host-a").unwrap().is_empty());
        complete_at(root.path(), &request, Outcome::Panels { panels: vec![] }).unwrap();
        let mut foreign = request.clone();
        foreign.actor = "horizon:other".into();
        assert!(take_result_at(root.path(), &foreign).is_err());
        assert!(take_result_at(root.path(), &request).unwrap().is_some());
        assert!(take_result_at(root.path(), &request).unwrap().is_none());
    }

    #[test]
    fn unbound_callers_and_queue_overflow_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        for identity in [
            AgentIdentity::new("outside", Some("host-a")),
            AgentIdentity::new("horizon:agent", None),
        ] {
            assert!(enqueue_at(root.path(), identity, Operation::List, Duration::ZERO).is_err());
        }
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        for _ in 0..MAX_PENDING_REQUESTS {
            enqueue_at(root.path(), identity, Operation::List, Duration::ZERO).unwrap();
        }
        assert_eq!(
            enqueue_at(root.path(), identity, Operation::List, Duration::ZERO)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn endpoint_only_requests_remain_valid_and_identity_ips_are_typed() {
        let legacy: Operation = serde_json::from_str(r#"{"operation":"create","endpoint":"127.0.0.1:5900"}"#).unwrap();
        assert!(matches!(legacy, Operation::Create { identity: None, .. }));
        let input = r#"{"operation":"create","endpoint":"127.0.0.1:5900","identity":{"hostname":"lab-host","ip_addresses":["192.0.2.1","2001:db8::1"]}}"#;
        let request: Operation = serde_json::from_str(input).unwrap();
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["identity"]["hostname"], "lab-host");
        assert_eq!(encoded["identity"]["ip_addresses"][1], "2001:db8::1");
        assert!(serde_json::from_str::<Operation>(&input.replace("192.0.2.1", "not-an-ip")).is_err());
    }
}
