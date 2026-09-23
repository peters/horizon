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
        /// Numeric loopback address and nonzero port of the VNC server, as seen
        /// from this machine or, with `ssh`, from that SSH host.
        endpoint: String,
        /// Optional labels supplied by the session creator, not verified by VNC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<DeviceIdentity>,
        /// Reach `endpoint` on another machine's loopback through `ssh -W`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ssh: Option<SshRoute>,
    },
    List,
    Inspect {
        panel_id: String,
    },
    Visibility {
        panel_id: String,
        visible: bool,
    },
    /// Bring an owned viewer into view without reconnecting or claiming image readiness.
    Reveal {
        panel_id: String,
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

/// An SSH host whose loopback holds the VNC server. Horizon runs `ssh -W`
/// with the host machine's own SSH configuration and keys; the route never
/// carries credentials, identity files or extra arguments.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshRoute {
    /// Host name, address or SSH config alias. Required.
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// SSH port; omitted means the SSH configuration's or 22.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl SshRoute {
    /// A host name is at most 253 characters; aliases and addresses are shorter.
    pub const MAX_HOST_CHARS: usize = 253;
    /// Longer than any login name a system accepts.
    pub const MAX_USER_CHARS: usize = 64;

    /// Trim the labels and keep them to the characters a host name, address,
    /// SSH config alias or user name is made of. `ssh` passes `%h` and `%r`
    /// into shell-executed `ProxyCommand` and `Match exec` lines from the
    /// machine's own configuration, so a label is never allowed to carry
    /// shell metacharacters, options (leading `-`), whitespace or controls,
    /// and each label is bounded in length.
    ///
    /// # Errors
    /// Describes the first rejected field.
    pub fn normalize(&mut self) -> Result<(), String> {
        fn label(value: &str, what: &str, extra: &[char], max_chars: usize) -> Result<String, String> {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(format!("ssh.{what} cannot be empty"));
            }
            if trimmed.len() > max_chars {
                return Err(format!("ssh.{what} must be at most {max_chars} characters"));
            }
            let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') || extra.contains(&c);
            if trimmed.starts_with('-') || !trimmed.chars().all(plain) {
                return Err(format!(
                    "ssh.{what} may only contain letters, digits, '.', '_' and '-'{}, and cannot start with '-'",
                    if extra.is_empty() { "" } else { " (and ':' for IPv6)" }
                ));
            }
            Ok(trimmed.to_owned())
        }
        self.host = label(&self.host, "host", &[':'], Self::MAX_HOST_CHARS)?;
        if let Some(user) = &self.user {
            self.user = Some(label(user, "user", &[], Self::MAX_USER_CHARS)?);
        }
        if self.port == Some(0) {
            return Err("ssh.port must be nonzero".into());
        }
        Ok(())
    }
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
    /// The SSH host `endpoint` is reached through, when tunnelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshRoute>,
    #[serde(default)]
    pub server: DeviceServerDetails,
    pub visible: bool,
    pub owned_by_caller: bool,
    pub connection: Connection,
    pub connection_error: Option<String>,
    /// Absent on older hosts; lack of diagnostics is not proof of a stalled stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Diagnostics>,
    #[serde(flatten)]
    pub image: ImageEvidence,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    Stopped,
    Connecting,
    Disconnected,
    Hidden,
    NotRendered,
    AwaitingFrame,
    Clipped,
    Displayed,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Diagnostics {
    pub observed_at_millis: i64,
    pub connection_generation: u64,
    pub presentation: Presentation,
    /// Legacy pause signal. Current viewers keep sampling while hidden or off canvas.
    pub sampling_paused: bool,
    /// Decoded frames, independent of texture uploads and rendering.
    pub decoded_frame_sequence: u64,
    pub last_decoded_age_millis: Option<u64>,
    /// Last received-frame texture submission; repainting retained pixels does not refresh it.
    #[serde(default)]
    pub last_uploaded_age_millis: Option<u64>,
    pub last_displayed_age_millis: Option<u64>,
    /// Last completed host pass, separate from transport and actual image evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostPresentation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostViewport {
    Root,
    Detached,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostExclusion {
    Hidden,
    OtherPanelFullscreen,
    OtherCloudFullscreen,
    OutsideCanvas,
    HostOverlay,
    DetachedViewportNotRendered,
    Unclassified,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct HostCanvas {
    pub pan_offset: [f32; 2],
    pub zoom: f32,
    /// Canvas bounds [left, top, right, bottom] in viewport points.
    pub rect: [f32; 4],
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct HostPresentation {
    pub observed_at_millis: i64,
    pub viewport: HostViewport,
    /// Why the host omitted this viewer, if known. None does not prove an image.
    pub exclusion: Option<HostExclusion>,
    pub canvas: Option<HostCanvas>,
    /// Navigation after panel rendering may already have changed the next view.
    pub canvas_after_pass: Option<HostCanvas>,
    /// Viewport-local egui pass number, not a platform presentation counter.
    pub ui_pass: u64,
    /// `will_discard` sampled at UI callback completion, before end-pass plugins.
    pub discarded: bool,
    /// Changes to the observed viewport/canvas, not an attribution to an actor.
    pub view_revision: u64,
    pub reveal_requests: u64,
    /// Most recent request applied to the canvas; dispatch alone is not application.
    pub applied_reveal_request: u64,
    /// Current view differs from the applied reveal; moving away and back resets it.
    /// None until both an applied reveal and a subsequent canvas observation exist.
    pub view_changed_since_reveal: Option<bool>,
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

/// Queue a request under an explicit runtime root.
///
/// # Errors
/// Rejects missing host identity, invalid actors, full queues and storage failures.
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
    fn create_accepts_an_ssh_route_and_older_requests_without_one() {
        let older: Operation =
            serde_json::from_value(serde_json::json!({"operation":"create","endpoint":"127.0.0.1:5900"})).unwrap();
        assert!(matches!(older, Operation::Create { ssh: None, .. }));
        let routed: Operation = serde_json::from_value(serde_json::json!({
            "operation":"create","endpoint":"127.0.0.1:5901",
            "ssh":{"host":"lab","user":"deploy","port":2222}
        }))
        .unwrap();
        let Operation::Create { ssh: Some(route), .. } = routed else {
            panic!("expected a routed create")
        };
        assert_eq!(route.host, "lab");
        assert_eq!(route.user.as_deref(), Some("deploy"));
        assert_eq!(route.port, Some(2222));
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation":"create","endpoint":"127.0.0.1:5901","ssh":{"host":"lab","identity_file":"~/.ssh/id"}
            }))
            .is_err(),
            "credentials and key paths are not part of the route"
        );
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation":"create","endpoint":"127.0.0.1:5901","ssh":{"user":"deploy"}
            }))
            .is_err(),
            "a route without a host is rejected at the schema, not defaulted"
        );
        let schema = serde_json::to_value(schemars::schema_for!(SshRoute)).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["host"]));
    }

    #[test]
    fn ssh_routes_are_trimmed_and_option_like_or_broken_labels_are_refused() {
        let mut route = SshRoute {
            host: "  lab.example  ".into(),
            user: Some(" deploy ".into()),
            port: Some(2222),
        };
        route.normalize().unwrap();
        assert_eq!(
            (route.host.as_str(), route.user.as_deref()),
            ("lab.example", Some("deploy"))
        );
        for host in [
            "lab",
            "lab-01.example.ts.net",
            "192.0.2.10",
            "fd7a:115c::1",
            "under_score",
        ] {
            let mut route = SshRoute {
                host: host.into(),
                ..Default::default()
            };
            assert!(route.normalize().is_ok(), "{host}");
        }
        // `%h` and `%r` reach shell-executed ProxyCommand and Match exec lines.
        for (host, user, port, field) in [
            ("  ", None, None, "ssh.host cannot be empty"),
            ("-oProxyCommand=id", None, None, "ssh.host may only"),
            ("lab example", None, None, "ssh.host may only"),
            ("lab\u{7}", None, None, "ssh.host may only"),
            ("lab;id", None, None, "ssh.host may only"),
            ("lab$(id)", None, None, "ssh.host may only"),
            ("lab`id`", None, None, "ssh.host may only"),
            ("lab>out", None, None, "ssh.host may only"),
            ("lab|id", None, None, "ssh.host may only"),
            ("lab%h", None, None, "ssh.host may only"),
            ("user@lab", None, None, "ssh.host may only"),
            ("lab/", None, None, "ssh.host may only"),
            ("lab", Some("-l root"), None, "ssh.user may only"),
            ("lab", Some("deploy;id"), None, "ssh.user may only"),
            ("lab", Some("a:b"), None, "ssh.user may only"),
            ("lab", Some(" "), None, "ssh.user cannot be empty"),
            ("lab", None, Some(0), "ssh.port must be nonzero"),
        ] {
            let mut route = SshRoute {
                host: host.into(),
                user: user.map(str::to_owned),
                port,
            };
            let error = route.normalize().unwrap_err();
            assert!(error.starts_with(field), "{host:?} {user:?} {port:?}: {error}");
        }
        let long_host = "h".repeat(SshRoute::MAX_HOST_CHARS);
        let long_user = "u".repeat(SshRoute::MAX_USER_CHARS);
        let mut longest = SshRoute {
            host: format!(" {long_host} "),
            user: Some(long_user.clone()),
            port: None,
        };
        assert!(longest.normalize().is_ok(), "bounds are inclusive after trimming");
        for (host, user, field) in [
            (format!("{long_host}h"), None, "ssh.host must be at most 253"),
            (
                "lab".to_owned(),
                Some(format!("{long_user}u")),
                "ssh.user must be at most 64",
            ),
        ] {
            let mut route = SshRoute { host, user, port: None };
            let error = route.normalize().unwrap_err();
            assert!(error.starts_with(field), "{error}");
        }
        for (host, user, port, field) in [
            ("lab\u{7}", None, None, "ssh.host may only"),
            ("lab", Some("deploy;id"), None, "ssh.user may only"),
        ] {
            let mut route = SshRoute {
                host: host.into(),
                user: user.map(str::to_owned),
                port,
            };
            let error = route.normalize().unwrap_err();
            assert!(error.starts_with(field), "{host:?} {user:?} {port:?}: {error}");
        }
    }

    #[test]
    fn older_diagnostics_do_not_imply_a_texture_upload_time() {
        let diagnostics: Diagnostics = serde_json::from_value(serde_json::json!({
            "observed_at_millis":0,"connection_generation":1,"presentation":"displayed",
            "sampling_paused":false,"decoded_frame_sequence":1,
            "last_decoded_age_millis":10,"last_displayed_age_millis":0
        }))
        .unwrap();
        assert!(diagnostics.last_uploaded_age_millis.is_none());
        assert!(diagnostics.host.is_none());
    }

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
        assert!(!has_pending_at(root.path(), "host-a").unwrap());
        let request = enqueue_at(root.path(), identity, Operation::List, Duration::from_secs(5)).unwrap();
        assert!(has_pending_at(root.path(), "host-a").unwrap());
        assert!(!has_pending_at(root.path(), "host-b").unwrap());
        assert!(claim_at(root.path(), "host-b").unwrap().is_empty());
        assert!(has_pending_at(root.path(), "host-a").unwrap());
        assert_eq!(claim_at(root.path(), "host-a").unwrap().len(), 1);
        assert!(!has_pending_at(root.path(), "host-a").unwrap());
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
