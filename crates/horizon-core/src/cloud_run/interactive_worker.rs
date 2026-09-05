//! Provider-neutral lifecycle contract for persistent or time-limited interactive workers.

use super::{CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, WorkerTarget};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{error::Error, net::IpAddr};

/// Maximum explicitly selected time limit accepted by the common worker contract.
pub const MAX_INTERACTIVE_WORKER_LEASE_SECONDS: u32 = 30 * 24 * 60 * 60;
const INTERACTIVE_WORKER_LEASE_CLOCK_SKEW_SECONDS: i64 = 120;
const ED25519_BLOB_PREFIX: &[u8] = b"\0\0\0\x0bssh-ed25519\0\0\0\x20";

/// Inputs shared by every interactive-worker provider.
///
/// Provider-specific credentials and profile details belong to the provider
/// implementation and must not be persisted in this request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveWorkerRequest {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub target: WorkerTarget,
    /// Client public key retained for reconnect during this runtime generation.
    pub ssh_public_key: String,
}

impl InteractiveWorkerRequest {
    /// Check the provider-neutral fields before invoking a concrete provider.
    #[must_use]
    pub fn is_valid_for(&self, provider: CloudProvider) -> bool {
        valid_worker_target(&self.target, provider) && valid_ed25519_key(&self.ssh_public_key, true)
    }
}

/// Exact, provider-qualified ownership identity for one interactive worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractiveWorkerIdentity {
    pub provider: CloudProvider,
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    /// Opaque provider resource ID, never a display name or list position.
    pub resource_id: String,
}

impl InteractiveWorkerIdentity {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        valid_single_token(&self.resource_id, 2_048)
    }
}

/// Immutable expiry returned for an explicitly time-limited worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractiveWorkerLease {
    /// Exact RFC 3339 deadline supplied to the worker termination watchdog.
    pub terminate_after: String,
}

impl InteractiveWorkerLease {
    /// Check the durable timestamp shape without treating an expired worker as
    /// invalid for exact-resource cleanup.
    #[must_use]
    pub fn has_valid_shape(&self) -> bool {
        time::OffsetDateTime::parse(&self.terminate_after, &time::format_description::well_known::Rfc3339).is_ok()
    }

    pub(super) fn is_bounded_at(&self, lease_seconds: u32, observed_at: time::OffsetDateTime) -> bool {
        let Ok(deadline) =
            time::OffsetDateTime::parse(&self.terminate_after, &time::format_description::well_known::Rfc3339)
        else {
            return false;
        };
        let skew = time::Duration::seconds(INTERACTIVE_WORKER_LEASE_CLOCK_SKEW_SECONDS);
        let Some(earliest) = observed_at.checked_sub(skew) else {
            return false;
        };
        let Some(latest) = observed_at.checked_add(time::Duration::seconds(
            i64::from(lease_seconds) + INTERACTIVE_WORKER_LEASE_CLOCK_SKEW_SECONDS,
        )) else {
            return false;
        };
        (earliest..=latest).contains(&deadline)
    }
}

/// Observed execution lifetime. This is not a client or ownership lease.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "ObservedLifetimeSnapshot", into = "ObservedLifetimeSnapshot")]
pub enum InteractiveWorkerLifetime {
    Persistent,
    TimeLimited(InteractiveWorkerLease),
}

impl InteractiveWorkerLifetime {
    #[must_use]
    pub const fn as_time_limited(&self) -> Option<&InteractiveWorkerLease> {
        match self {
            Self::Persistent => None,
            Self::TimeLimited(lease) => Some(lease),
        }
    }

    #[must_use]
    pub fn has_valid_shape(&self) -> bool {
        self.as_time_limited()
            .is_none_or(InteractiveWorkerLease::has_valid_shape)
    }

    pub(super) fn matches_policy(&self, policy: WorkerLifetime) -> bool {
        matches!(
            (self, policy),
            (Self::Persistent, WorkerLifetime::Persistent) | (Self::TimeLimited(_), WorkerLifetime::TimeLimited { .. })
        )
    }

    pub(super) fn is_valid_at(&self, policy: WorkerLifetime, observed_at: time::OffsetDateTime) -> bool {
        match (self, policy) {
            (Self::Persistent, WorkerLifetime::Persistent) => true,
            (Self::TimeLimited(lease), WorkerLifetime::TimeLimited { seconds }) => {
                lease.is_bounded_at(seconds, observed_at)
            }
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum ObservedLifetimeSnapshot {
    TimeLimited(InteractiveWorkerLease),
    Persistent { lifetime: PersistentObservedMarker },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistentObservedMarker {
    Persistent,
}

impl From<ObservedLifetimeSnapshot> for InteractiveWorkerLifetime {
    fn from(value: ObservedLifetimeSnapshot) -> Self {
        match value {
            ObservedLifetimeSnapshot::TimeLimited(lease) => Self::TimeLimited(lease),
            ObservedLifetimeSnapshot::Persistent { .. } => Self::Persistent,
        }
    }
}

impl From<InteractiveWorkerLifetime> for ObservedLifetimeSnapshot {
    fn from(value: InteractiveWorkerLifetime) -> Self {
        match value {
            InteractiveWorkerLifetime::TimeLimited(lease) => Self::TimeLimited(lease),
            InteractiveWorkerLifetime::Persistent => Self::Persistent {
                lifetime: PersistentObservedMarker::Persistent,
            },
        }
    }
}

/// Durable worker handle used for inspection and exact-resource cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractiveWorker {
    pub identity: InteractiveWorkerIdentity,
    /// Exact provider target used to create the resource.
    pub target: WorkerTarget,
    /// Client public key installed for this runtime generation's reconnects.
    pub ssh_public_key: String,
    /// The v1 wire field name remains `lease` to preserve bounded records.
    #[serde(rename = "lease")]
    pub lifetime: InteractiveWorkerLifetime,
}

impl InteractiveWorker {
    /// Check fields needed to safely persist, inspect, or delete this exact
    /// resource. Attachment additionally requires request- and time-bound
    /// validation through [`InteractiveWorkerStatus::is_ready_for`].
    #[must_use]
    pub fn has_valid_shape(&self) -> bool {
        self.identity.is_valid()
            && valid_worker_target(&self.target, self.identity.provider)
            && valid_ssh_public_key(&self.ssh_public_key)
            && self.lifetime.has_valid_shape()
            && self.lifetime.matches_policy(self.target.lifetime)
    }

    /// Check the durable handle before any provider-specific inspection or
    /// deletion call.
    #[must_use]
    pub fn is_valid_for(&self, provider: CloudProvider) -> bool {
        self.identity.provider == provider && self.has_valid_shape()
    }
}

/// Provider-neutral lifecycle observed for an exact worker resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveWorkerLifecycle {
    Provisioning,
    Ready,
    Stopped,
    Failed,
    Deleting,
    Unknown,
}

/// SSH data that is complete enough for a pinned interactive connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractiveWorkerSshEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// OpenSSH public host key (`algorithm base64`), obtained through a trusted path.
    pub host_key: String,
}

impl InteractiveWorkerSshEndpoint {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        valid_ssh_coordinates(&self.host, self.port, &self.username) && valid_ed25519_key(&self.host_key, false)
    }
}

/// Current state of an exact worker and its optional connection endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractiveWorkerStatus {
    pub worker: InteractiveWorker,
    pub lifecycle: InteractiveWorkerLifecycle,
    /// Present for `Ready`; providers must not report ready without a pinned host key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<InteractiveWorkerSshEndpoint>,
}

impl InteractiveWorkerStatus {
    /// A worker is attachable only when its lifecycle, request identity,
    /// complete target, lifetime, and SSH data agree. The target comparison covers
    /// provider, profile, image, disk, explicit lifetime policy, and cost limit.
    /// `observed_at` must be a freshly sampled, trusted UTC time from
    /// immediately before attachment.
    #[must_use]
    pub fn is_ready_for(&self, request: &InteractiveWorkerRequest, observed_at: time::OffsetDateTime) -> bool {
        let identity = &self.worker.identity;
        request.is_valid_for(request.target.provider)
            && matches!(self.lifecycle, InteractiveWorkerLifecycle::Ready)
            && self.worker.is_valid_for(request.target.provider)
            && identity.workflow_id == request.workflow_id
            && identity.job_id == request.job_id
            && self.worker.target == request.target
            && self.worker.ssh_public_key == request.ssh_public_key
            && self.worker.lifetime.is_valid_at(request.target.lifetime, observed_at)
            && self.ssh.as_ref().is_some_and(InteractiveWorkerSshEndpoint::is_complete)
    }
}

/// Whether an idempotent ensure call created or recovered the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerEnsure {
    Created(InteractiveWorkerStatus),
    Reused(InteractiveWorkerStatus),
}

impl InteractiveWorkerEnsure {
    #[must_use]
    pub const fn status(&self) -> &InteractiveWorkerStatus {
        match self {
            Self::Created(status) | Self::Reused(status) => status,
        }
    }

    #[must_use]
    pub fn into_status(self) -> InteractiveWorkerStatus {
        match self {
            Self::Created(status) | Self::Reused(status) => status,
        }
    }
}

/// Idempotent cleanup result for one exact worker resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerCleanup {
    Deleted,
    AlreadyAbsent,
}

/// Blocking provider boundary for one interactive worker.
///
/// Callers must invoke provider operations away from the render thread. An
/// implementation must reject an ensure request unless
/// [`InteractiveWorkerRequest::is_valid_for`] accepts [`Self::provider`]. It
/// must also reject inspect and delete operations before provider I/O unless
/// [`InteractiveWorker::is_valid_for`] accepts [`Self::provider`]. Valid
/// operations must make `ensure_worker` idempotent across controllers,
/// preserve the exact returned handle for later inspection, and delete only
/// that exact resource.
/// Providers must reject unsupported lifetime policies before provider I/O;
/// they must not silently convert a persistent request to a time-limited job.
pub trait InteractiveWorkerProvider: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    #[must_use]
    fn provider(&self) -> CloudProvider;

    /// Create once or recover the single exact worker for this request.
    ///
    /// # Errors
    /// Returns a redacted, provider-specific error when the operation cannot
    /// safely produce an exact worker identity.
    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error>;

    /// Inspect only the exact persisted worker, returning `None` when absent.
    ///
    /// # Errors
    /// Returns a redacted, provider-specific error before provider I/O when
    /// the worker is malformed or belongs to another provider, or when state
    /// is ambiguous.
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error>;

    /// Delete only the exact persisted worker. Repeated deletion is idempotent.
    ///
    /// # Errors
    /// Returns a redacted, provider-specific error before provider I/O when
    /// the worker is malformed or belongs to another provider, or unless
    /// absence is verified.
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error>;
}

pub(crate) fn valid_worker_target(target: &WorkerTarget, provider: CloudProvider) -> bool {
    target.provider == provider
        && !target.profile.trim().is_empty()
        && target.profile.trim() == target.profile
        && target.profile.len() <= 191
        && !target.profile.chars().any(char::is_control)
        && valid_immutable_image(&target.image)
        && target.disk_gib > 0
        && target.lifetime.is_valid()
        && target
            .lifetime
            .time_limit_seconds()
            .is_none_or(|seconds| seconds <= MAX_INTERACTIVE_WORKER_LEASE_SECONDS)
        && target.max_hourly_cost_micros != Some(0)
}

fn valid_immutable_image(value: &str) -> bool {
    super::validation::valid_worker_image(value)
        && value
            .rsplit_once("@sha256:")
            .is_some_and(|(_, digest)| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn valid_single_token(value: &str, maximum_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_length
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn valid_ssh_host(value: &str) -> bool {
    if !valid_single_token(value, 253) {
        return false;
    }
    value.parse::<IpAddr>().is_ok() || value.split('.').all(valid_dns_label)
}

pub(super) fn valid_ssh_coordinates(host: &str, port: u16, username: &str) -> bool {
    valid_ssh_host(host) && port > 0 && valid_ssh_username(username)
}

fn valid_dns_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_ssh_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn valid_ssh_public_key(value: &str) -> bool {
    valid_ed25519_key(value, true)
}

fn valid_ed25519_key(value: &str, allow_comment: bool) -> bool {
    if value.is_empty() || value.len() > 16 * 1_024 || value.trim() != value || value.chars().any(char::is_control) {
        return false;
    }
    let mut fields = value.split_ascii_whitespace();
    let (Some("ssh-ed25519"), Some(encoded)) = (fields.next(), fields.next()) else {
        return false;
    };
    let valid_payload = STANDARD.decode(encoded).is_ok_and(|decoded| {
        decoded.len() == ED25519_BLOB_PREFIX.len() + 32 && decoded.starts_with(ED25519_BLOB_PREFIX)
    });
    valid_payload && (allow_comment || fields.next().is_none())
}

#[cfg(test)]
mod lifetime_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq, thiserror::Error)]
    enum FakeError {
        #[error("invalid request")]
        InvalidRequest,
        #[error("invalid worker")]
        InvalidWorker,
    }

    #[derive(Clone)]
    struct FakeProvider {
        status: InteractiveWorkerStatus,
    }

    impl InteractiveWorkerProvider for FakeProvider {
        type Error = FakeError;

        fn provider(&self) -> CloudProvider {
            CloudProvider::RunPod
        }

        fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
            if !request.is_valid_for(self.provider()) {
                return Err(FakeError::InvalidRequest);
            }
            Ok(InteractiveWorkerEnsure::Created(self.status.clone()))
        }

        fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
            if !worker.is_valid_for(self.provider()) {
                return Err(FakeError::InvalidWorker);
            }
            Ok(Some(self.status.clone()))
        }

        fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
            if !worker.is_valid_for(self.provider()) {
                return Err(FakeError::InvalidWorker);
            }
            Ok(InteractiveWorkerCleanup::Deleted)
        }
    }

    fn ed25519_key(comment: Option<&str>) -> String {
        let mut blob = ED25519_BLOB_PREFIX.to_vec();
        blob.extend([42; 32]);
        let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
        if let Some(comment) = comment {
            format!("{key} {comment}")
        } else {
            key
        }
    }

    pub(super) fn worker_status() -> InteractiveWorkerStatus {
        InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::RunPod,
                    workflow_id: CloudWorkflowId::new(),
                    job_id: CloudJobId::new(),
                    resource_id: "pod-123".to_string(),
                },
                target: WorkerTarget {
                    provider: CloudProvider::RunPod,
                    profile: "gpu".to_string(),
                    image: format!("registry.example/horizon/worker@sha256:{}", "d".repeat(64)),
                    disk_gib: 20,
                    lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
                    max_hourly_cost_micros: Some(500_000),
                },
                ssh_public_key: ed25519_key(None),
                lifetime: InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                    terminate_after: "2026-09-04T12:00:00Z".to_string(),
                }),
            },
            lifecycle: InteractiveWorkerLifecycle::Ready,
            ssh: Some(InteractiveWorkerSshEndpoint {
                host: "worker.example".to_string(),
                port: 22,
                username: "root".to_string(),
                host_key: ed25519_key(None),
            }),
        }
    }

    fn worker_request(status: &InteractiveWorkerStatus) -> InteractiveWorkerRequest {
        InteractiveWorkerRequest {
            workflow_id: status.worker.identity.workflow_id,
            job_id: status.worker.identity.job_id,
            target: status.worker.target.clone(),
            ssh_public_key: status.worker.ssh_public_key.clone(),
        }
    }

    fn observed_at() -> time::OffsetDateTime {
        time::OffsetDateTime::parse("2026-09-04T11:50:00Z", &time::format_description::well_known::Rfc3339)
            .expect("observation timestamp")
    }

    #[test]
    fn provider_trait_is_object_safe_and_preserves_exact_status() {
        let status = worker_status();
        let provider = FakeProvider { status: status.clone() };
        let provider: &dyn InteractiveWorkerProvider<Error = FakeError> = &provider;
        let request = worker_request(&status);

        assert_eq!(provider.provider(), CloudProvider::RunPod);
        assert!(request.is_valid_for(provider.provider()));
        let ensured = provider.ensure_worker(&request).expect("fake ensure");
        assert_eq!(ensured.status(), &status);
        assert_eq!(ensured.into_status(), status);
        assert_eq!(provider.inspect_worker(&status.worker), Ok(Some(status.clone())));
        assert_eq!(
            provider.delete_worker(&status.worker),
            Ok(InteractiveWorkerCleanup::Deleted)
        );

        let mut wrong_provider = status.worker;
        wrong_provider.identity.provider = CloudProvider::Azure;
        assert_eq!(provider.inspect_worker(&wrong_provider), Err(FakeError::InvalidWorker));
        assert_eq!(provider.delete_worker(&wrong_provider), Err(FakeError::InvalidWorker));
    }

    #[test]
    fn ready_requires_complete_ssh_data() {
        let mut status = worker_status();
        let request = worker_request(&status);
        assert!(status.is_ready_for(&request, observed_at()));

        status.ssh = None;
        assert!(!status.is_ready_for(&request, observed_at()));
        status.lifecycle = InteractiveWorkerLifecycle::Provisioning;
        status.ssh = worker_status().ssh;
        assert!(!status.is_ready_for(&request, observed_at()));

        status.lifecycle = InteractiveWorkerLifecycle::Ready;
        status.ssh.as_mut().expect("SSH endpoint").port = 0;
        assert!(!status.is_ready_for(&request, observed_at()));
        status.ssh = worker_status().ssh;
        status.ssh.as_mut().expect("SSH endpoint").host_key = "ssh-rsa unsupported".to_string();
        assert!(!status.is_ready_for(&request, observed_at()));
        status.ssh = worker_status().ssh;
        status.ssh.as_mut().expect("SSH endpoint").host = "-oProxyCommand=bad".to_string();
        assert!(!status.is_ready_for(&request, observed_at()));

        for host in [".", "[", ":::", "[2001:db8::1]", "bad_host"] {
            status.ssh = worker_status().ssh;
            status.ssh.as_mut().expect("SSH endpoint").host = host.to_string();
            assert!(
                !status.is_ready_for(&request, observed_at()),
                "accepted invalid host {host}"
            );
        }
        status.ssh = worker_status().ssh;
        status.ssh.as_mut().expect("SSH endpoint").host = "2001:db8::1".to_string();
        assert!(status.is_ready_for(&request, observed_at()));
    }

    #[test]
    fn ready_rejects_request_drift_and_unbounded_deadlines() {
        let original = worker_status();
        let request = worker_request(&original);

        let mut status = original.clone();
        status.worker.identity.provider = CloudProvider::Azure;
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original.clone();
        status.worker.identity.workflow_id = CloudWorkflowId::new();
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original.clone();
        status.worker.identity.job_id = CloudJobId::new();
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original.clone();
        status.worker.target.image = format!("registry.example/horizon/worker@sha256:{}", "a".repeat(64));
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original.clone();
        status.worker.target.disk_gib += 1;
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original.clone();
        status.worker.ssh_public_key = {
            let mut blob = ED25519_BLOB_PREFIX.to_vec();
            blob.extend([43; 32]);
            format!("ssh-ed25519 {}", STANDARD.encode(blob))
        };
        assert!(!status.is_ready_for(&request, observed_at()));
        status = original;
        status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: "2999-09-04T12:00:00Z".to_string(),
        });
        assert!(!status.is_ready_for(&request, observed_at()));
    }

    #[test]
    fn request_preflight_rejects_provider_drift_and_mutable_images() {
        let status = worker_status();
        let mut request = InteractiveWorkerRequest {
            workflow_id: status.worker.identity.workflow_id,
            job_id: status.worker.identity.job_id,
            target: status.worker.target,
            ssh_public_key: ed25519_key(Some("user@example")),
        };
        assert!(request.is_valid_for(CloudProvider::RunPod));
        assert!(!request.is_valid_for(CloudProvider::Azure));

        request.target.image = "registry.example/horizon/worker:latest".to_string();
        assert!(!request.is_valid_for(CloudProvider::RunPod));
        request.target.image = format!("registry.example/horizon/worker@sha256:{}", "d".repeat(64));
        request.ssh_public_key = "ssh-rsa unsupported".to_string();
        assert!(!request.is_valid_for(CloudProvider::RunPod));
        request.ssh_public_key = ed25519_key(None);
        request.target.lifetime = WorkerLifetime::TimeLimited {
            seconds: MAX_INTERACTIVE_WORKER_LEASE_SECONDS + 1,
        };
        assert!(!request.is_valid_for(CloudProvider::RunPod));
        request.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        request.target.max_hourly_cost_micros = Some(0);
        assert!(!request.is_valid_for(CloudProvider::RunPod));
    }

    #[test]
    fn persisted_handle_rejects_ambiguous_identity_and_lease() {
        let mut worker = worker_status().worker;
        assert!(worker.has_valid_shape());
        assert!(worker.is_valid_for(CloudProvider::RunPod));
        assert!(!worker.is_valid_for(CloudProvider::Azure));

        worker.identity.resource_id = "display name".to_string();
        assert!(!worker.has_valid_shape());
        assert!(!worker.is_valid_for(CloudProvider::RunPod));
        worker = worker_status().worker;
        worker.target.image = "registry.example/horizon/worker:latest".to_string();
        assert!(!worker.has_valid_shape());
        worker = worker_status().worker;
        worker.ssh_public_key = "ssh-rsa unsupported".to_string();
        assert!(!worker.has_valid_shape());
        worker = worker_status().worker;
        worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: "in two hours".to_string(),
        });
        assert!(!worker.has_valid_shape());
    }

    #[test]
    fn durable_worker_status_round_trips_with_stable_wire_values() {
        let status = worker_status();
        let encoded = serde_json::to_string(&status).expect("serialize status");
        assert!(encoded.contains("\"lifecycle\":\"ready\""));
        assert!(encoded.contains("\"host_key\":\"ssh-ed25519"));
        assert!(encoded.contains("\"ssh_public_key\":\"ssh-ed25519"));
        let decoded = serde_json::from_str::<InteractiveWorkerStatus>(&encoded).expect("deserialize status");
        assert_eq!(decoded, status);

        let unknown = encoded.replacen("\"resource_id\"", "\"unexpected\":true,\"resource_id\"", 1);
        assert!(serde_json::from_str::<InteractiveWorkerStatus>(&unknown).is_err());
    }
}
