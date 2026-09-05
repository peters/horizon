//! `RunPod` Secure Cloud lifecycle adapter for durable cloud workers.
use super::{
    CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, WorkerTarget,
    interactive_worker::{InteractiveWorkerLease, InteractiveWorkerLifetime, valid_ssh_public_key},
    validation::valid_worker_image,
};
use serde::{Deserialize, Deserializer, de};
use std::{collections::BTreeMap, env, fmt};
mod create_request;
mod http;
mod interactive;
mod models;
#[cfg(test)]
mod tests;

use create_request::CreatePodRequest;
pub use interactive::{RunPodHostKeySource, RunPodInteractiveWorkerProvider};
pub use models::{
    RunPodCleanup, RunPodEnsure, RunPodError, RunPodLifecycle, RunPodProfile, RunPodSshEndpoint, RunPodWorker,
    RunPodWorkerStatus,
};

const WORKFLOW_ENV: &str = "HORIZON_WORKFLOW_ID";
const JOB_ENV: &str = "HORIZON_JOB_ID";
const PROTOCOL_ENV: &str = "HORIZON_CLOUD_PROTOCOL_VERSION";
const SSH_PUBLIC_KEY_ENV: &str = "HORIZON_SSH_PUBLIC_KEY";
const TERMINATE_ENV: &str = "HORIZON_TERMINATE_AFTER";
const LIFETIME_ENV: &str = "HORIZON_WORKER_LIFETIME";
const PERSISTENT_LIFETIME: &str = "persistent";
const MIN_LEASE_SECONDS: u32 = 300;
const MAX_LEASE_SECONDS: u32 = 30 * 24 * 60 * 60;
#[derive(Clone)]
pub struct RunPodApiKey(String);
impl RunPodApiKey {
    /// Validate a token supplied by a secret store.
    /// # Errors
    pub fn new(value: impl Into<String>) -> Result<Self, RunPodError> {
        let value = value.into();
        let valid = !value.is_empty() && value.len() <= 4_096 && value.bytes().all(|byte| byte.is_ascii_graphic());
        valid.then_some(Self(value)).ok_or(RunPodError::InvalidApiKey)
    }
    /// Load the token injected by the control plane.
    /// # Errors
    pub fn from_env() -> Result<Self, RunPodError> {
        env::var("RUNPOD_API_KEY")
            .map_err(|_| RunPodError::MissingApiKey)
            .and_then(Self::new)
    }
    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for RunPodApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RunPodApiKey(<redacted>)")
    }
}
/// Durable, cross-controller compare-and-set whose claim survives process exit.
/// A resource name may return `true` at most once.
pub trait RunPodCreationFence: Send + Sync {
    /// # Errors
    /// Returns an error when the durable claim cannot be read or recorded.
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_name: &str,
    ) -> Result<bool, RunPodError>;
}
impl<F> RunPodCreationFence for F
where
    F: Fn(CloudWorkflowId, CloudJobId, &WorkerTarget, &str) -> Result<bool, RunPodError> + Send + Sync,
{
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_name: &str,
    ) -> Result<bool, RunPodError> {
        self(workflow_id, job_id, target, resource_name)
    }
}
impl RunPodCreationFence for super::CloudWorkflowStore {
    fn claim_once(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_name: &str,
    ) -> Result<bool, RunPodError> {
        self.claim_worker_creation(workflow_id, job_id, target, resource_name)
            .map_err(|error| RunPodError::CreationFenceFailed {
                reason: error.to_string(),
            })
    }
}
pub struct RunPodClient {
    transport: Box<dyn Transport>,
    creation_fence: Box<dyn RunPodCreationFence>,
}
impl RunPodClient {
    /// Build a production client pinned to `RunPod`'s HTTPS control planes.
    pub fn new(api_key: &RunPodApiKey, creation_fence: impl RunPodCreationFence + 'static) -> Self {
        Self {
            transport: Box::new(http::RunPodHttp::new(api_key)),
            creation_fence: Box::new(creation_fence),
        }
    }
    /// Create a worker once, or adopt the single exact resource from an interrupted attempt.
    /// # Errors
    pub fn ensure_worker(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
    ) -> Result<RunPodEnsure, RunPodError> {
        self.ensure_worker_with_ssh_public_key(workflow_id, job_id, target, profile, None)
    }

    fn ensure_interactive_worker(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
        ssh_public_key: &str,
    ) -> Result<RunPodEnsure, RunPodError> {
        self.ensure_worker_with_ssh_public_key(workflow_id, job_id, target, profile, Some(ssh_public_key))
    }

    fn ensure_worker_with_ssh_public_key(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
        ssh_public_key: Option<&str>,
    ) -> Result<RunPodEnsure, RunPodError> {
        validate_target(target, profile)?;
        if ssh_public_key.is_some_and(|key| !valid_ssh_public_key(key)) {
            return Err(RunPodError::InvalidTarget);
        }
        let name = resource_name(workflow_id, job_id);
        let matches = self.reconcile_by_name(&name)?;
        let may_create = !matches.is_empty() || self.creation_fence.claim_once(workflow_id, job_id, target, &name)?;
        let (pod, created, expected_deadline) = match matches.as_slice() {
            [] if !may_create => return Err(RunPodError::CreationUnresolved { name }),
            [] => {
                let request =
                    CreatePodRequest::new(workflow_id, job_id, target, profile, name.clone(), ssh_public_key)?;
                let expected_deadline = request.terminate_after.clone();
                (self.create_worker(&request)?, true, expected_deadline)
            }
            [pod] => (pod.clone(), false, None),
            _ => {
                return Err(RunPodError::AmbiguousResource {
                    name,
                    count: matches.len(),
                });
            }
        };
        let status = match status_from_pod(
            &pod,
            workflow_id,
            job_id,
            target,
            expected_deadline.as_deref(),
            ssh_public_key,
        ) {
            Ok(status) => status,
            Err(_) if created && target.lifetime == WorkerLifetime::Persistent => {
                return Err(RunPodError::PersistentCreationReconciliationRequired { name, pod_id: pod.id });
            }
            Err(error) if created => {
                if self.transport.delete(&pod.id).is_err() {
                    return Err(RunPodError::CreationCleanupFailed { pod_id: pod.id });
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if !created
            && !status
                .worker
                .lifetime
                .is_valid_at(target.lifetime, time::OffsetDateTime::now_utc())
        {
            let worker = Box::new(status.worker);
            return if self.delete_worker(&worker).is_ok() {
                Err(RunPodError::LeaseDeadlineRejected { worker })
            } else {
                Err(RunPodError::LeaseRejectionCleanupFailed { worker })
            };
        }
        self.enforce_cost_limit(&status.worker, target.max_hourly_cost_micros)?;
        Ok(if created {
            RunPodEnsure::Created(status)
        } else {
            RunPodEnsure::Reused(status)
        })
    }
    fn create_worker(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        self.transport.create(request).map_err(|error| {
            if request.terminate_after.is_none()
                && !matches!(error, RunPodError::PersistentCreationReconciliationRequired { .. })
            {
                RunPodError::PersistentCreationUnresolved {
                    name: request.name.clone(),
                    cause: Box::new(error),
                }
            } else {
                error
            }
        })
    }
    fn reconcile_by_name(&self, name: &str) -> Result<Vec<ApiPod>, RunPodError> {
        let mut matches = BTreeMap::new();
        let mut observe = || {
            for pod in self.transport.list_by_name(name)? {
                if !valid_provider_id(&pod.id) {
                    return Err(RunPodError::ResourceIdentityMismatch);
                }
                matches.insert(pod.id.clone(), pod);
            }
            Ok(())
        };
        observe()?;
        for delay_ms in http::PROPAGATION_BACKOFF_MS {
            if !cfg!(test) {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            observe()?;
        }
        Ok(matches.into_values().collect())
    }
    /// Inspect an exact persisted worker, returning `None` after provider deletion.
    /// # Errors
    pub fn inspect_worker(&self, worker: &RunPodWorker) -> Result<Option<RunPodWorkerStatus>, RunPodError> {
        worker.validate()?;
        self.transport
            .get(&worker.pod_id)?
            .map(|pod| status_from_resource(&pod, worker, None))
            .transpose()
    }

    fn inspect_interactive_worker(
        &self,
        worker: &RunPodWorker,
        ssh_public_key: &str,
    ) -> Result<Option<RunPodWorkerStatus>, RunPodError> {
        worker.validate()?;
        if !valid_ssh_public_key(ssh_public_key) {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        self.transport
            .get(&worker.pod_id)?
            .map(|pod| status_from_resource(&pod, worker, Some(ssh_public_key)))
            .transpose()
    }
    /// Delete only the exact resource proven to belong to the persisted workflow and job.
    /// # Errors
    pub fn delete_worker(&self, worker: &RunPodWorker) -> Result<RunPodCleanup, RunPodError> {
        worker.validate()?;
        let Some(pod) = self.transport.get(&worker.pod_id)? else {
            return Ok(RunPodCleanup::AlreadyAbsent);
        };
        status_from_resource(&pod, worker, None)?;
        self.transport.delete(&worker.pod_id)
    }

    fn delete_interactive_worker(
        &self,
        worker: &RunPodWorker,
        ssh_public_key: &str,
    ) -> Result<RunPodCleanup, RunPodError> {
        worker.validate()?;
        if !valid_ssh_public_key(ssh_public_key) {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        let Some(pod) = self.transport.get(&worker.pod_id)? else {
            return Ok(RunPodCleanup::AlreadyAbsent);
        };
        status_from_resource(&pod, worker, Some(ssh_public_key))?;
        self.transport.delete(&worker.pod_id)
    }
    fn enforce_cost_limit(&self, worker: &RunPodWorker, maximum: Option<u64>) -> Result<(), RunPodError> {
        let Some(maximum) = maximum else {
            return Ok(());
        };
        match worker.hourly_cost_micros {
            Some(actual) if actual <= maximum => Ok(()),
            actual if worker.lifetime == InteractiveWorkerLifetime::Persistent => {
                Err(RunPodError::PersistentWorkerCostRejected {
                    worker: Box::new(worker.clone()),
                    actual,
                    maximum,
                })
            }
            actual => self.cleanup_after_cost_rejection(worker, actual, maximum),
        }
    }
    fn cleanup_after_cost_rejection(
        &self,
        worker: &RunPodWorker,
        actual: Option<u64>,
        maximum: u64,
    ) -> Result<(), RunPodError> {
        self.delete_worker(worker)
            .map_err(|_| RunPodError::CostRejectionCleanupFailed {
                worker: Box::new(worker.clone()),
            })?;
        Err(RunPodError::HourlyCostRejected {
            worker: Box::new(worker.clone()),
            actual,
            maximum,
        })
    }
    #[cfg(test)]
    fn with_transport(transport: impl Transport + 'static) -> Self {
        Self {
            transport: Box::new(transport),
            creation_fence: Box::new(|_, _, _: &WorkerTarget, _: &str| Ok(true)),
        }
    }
}
trait Transport: Send + Sync {
    fn list_by_name(&self, name: &str) -> Result<Vec<ApiPod>, RunPodError>;
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError>;
    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError>;
    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError>;
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiPod {
    id: String,
    name: String,
    image: String,
    status: Option<String>,
    ssh: Option<ApiSsh>,
    env: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_hourly_cost")]
    cost: Option<u64>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ApiSsh {
    direct: Option<ApiSshEndpoint>,
}
#[derive(Clone, Debug, Deserialize)]
struct ApiSshEndpoint {
    username: String,
    host: String,
    port: u16,
}
fn validate_target(target: &WorkerTarget, profile: &RunPodProfile) -> Result<(), RunPodError> {
    let safe_text = |value: &str| {
        !value.is_empty() && value.len() <= 191 && value.trim() == value && !value.chars().any(char::is_control)
    };
    let safe_id_byte = |byte: u8| byte.is_ascii_alphanumeric() || b"._-".contains(&byte);
    let safe_id = |value: &str| safe_text(value) && value.bytes().all(safe_id_byte);
    let safe_port = |value: &str| {
        value.split_once('/').is_some_and(|(port, protocol)| {
            port.parse::<u16>().is_ok_and(|port| port > 0) && matches!(protocol, "tcp" | "http")
        })
    };
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    let valid_cuda = |value: &str| value.split('.').count() == 2 && value.split('.').all(digits);
    let nonzero = |value: Option<u32>| value.is_none_or(|value| value > 0);
    let valid = target.provider == CloudProvider::RunPod
        && target.profile == profile.name
        && target.disk_gib > 0
        && target
            .lifetime
            .time_limit_seconds()
            .is_none_or(|seconds| (MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&seconds))
        && target.max_hourly_cost_micros != Some(0)
        && valid_immutable_worker_image(&target.image)
        && safe_text(&profile.name)
        && !profile.gpu_type_ids.is_empty()
        && profile.gpu_type_ids.iter().all(|value| safe_text(value))
        && profile.gpu_count > 0
        && profile.allowed_cuda_versions.iter().all(|value| valid_cuda(value))
        && profile.data_center_id.as_deref().is_none_or(safe_id)
        && profile.ports.iter().all(|value| safe_port(value))
        && profile.ports.iter().any(|value| value == "22/tcp")
        && nonzero(profile.min_download_mbps)
        && nonzero(profile.min_upload_mbps)
        && nonzero(profile.min_disk_bandwidth_mbps)
        && profile.container_registry_auth_id.as_deref().is_none_or(safe_id);
    valid.then_some(()).ok_or(RunPodError::InvalidTarget)
}
fn status_from_pod(
    pod: &ApiPod,
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    expected_deadline: Option<&str>,
    ssh_public_key: Option<&str>,
) -> Result<RunPodWorkerStatus, RunPodError> {
    let lifetime = observed_lifetime(pod)?;
    if !lifetime.matches_policy(target.lifetime)
        || expected_deadline.is_some_and(|expected| {
            lifetime.as_time_limited().map(|lease| lease.terminate_after.as_str()) != Some(expected)
        })
    {
        return Err(RunPodError::ResourceIdentityMismatch);
    }
    let worker = RunPodWorker {
        workflow_id,
        job_id,
        pod_id: pod.id.clone(),
        name: resource_name(workflow_id, job_id),
        image: target.image.clone(),
        lifetime,
        hourly_cost_micros: pod.cost.filter(|cost| *cost > 0),
    };
    status_from_resource(pod, &worker, ssh_public_key)
}
fn status_from_resource(
    pod: &ApiPod,
    worker: &RunPodWorker,
    ssh_public_key: Option<&str>,
) -> Result<RunPodWorkerStatus, RunPodError> {
    worker.validate()?;
    let owned = pod.id == worker.pod_id
        && pod.name == worker.name
        && pod.image == worker.image
        && pod.env.get(WORKFLOW_ENV) == Some(&worker.workflow_id.to_string())
        && pod.env.get(JOB_ENV) == Some(&worker.job_id.to_string())
        && pod.env.get(PROTOCOL_ENV) == Some(&super::CLOUD_RUN_PROTOCOL_VERSION.to_string());
    let owned = owned
        && observed_lifetime(pod)? == worker.lifetime
        && ssh_public_key.is_none_or(|key| pod.env.get(SSH_PUBLIC_KEY_ENV).is_some_and(|value| value == key));
    if !owned {
        return Err(RunPodError::ResourceIdentityMismatch);
    }
    let status = pod.status.as_deref().map(str::to_ascii_uppercase);
    let lifecycle = match status.as_deref() {
        Some("PROVISIONING" | "STARTING") => RunPodLifecycle::Provisioning,
        Some("RUNNING") => RunPodLifecycle::Running,
        Some("EXITED") => RunPodLifecycle::Exited,
        Some("ERROR") => RunPodLifecycle::Failed,
        Some("TERMINATED") => RunPodLifecycle::Terminated,
        _ => RunPodLifecycle::Unknown,
    };
    let direct_ssh = pod.ssh.as_ref().and_then(|ssh| ssh.direct.as_ref());
    Ok(RunPodWorkerStatus {
        worker: RunPodWorker {
            hourly_cost_micros: pod.cost.filter(|cost| *cost > 0).or(worker.hourly_cost_micros),
            ..worker.clone()
        },
        lifecycle,
        ssh_username: direct_ssh.map(|endpoint| endpoint.username.clone()),
        ssh_host: direct_ssh.map(|endpoint| endpoint.host.clone()),
        ssh_port: direct_ssh.map(|endpoint| endpoint.port),
    })
}
fn resource_name(workflow_id: CloudWorkflowId, job_id: CloudJobId) -> String {
    format!("horizon-{workflow_id}-{job_id}")
}
fn observed_lifetime(pod: &ApiPod) -> Result<InteractiveWorkerLifetime, RunPodError> {
    match (
        pod.env.get(LIFETIME_ENV).map(String::as_str),
        pod.env.get(TERMINATE_ENV),
    ) {
        (Some(PERSISTENT_LIFETIME), None) => Ok(InteractiveWorkerLifetime::Persistent),
        (None, Some(deadline)) => Ok(InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: deadline.clone(),
        })),
        _ => Err(RunPodError::ResourceIdentityMismatch),
    }
}
fn termination_deadline(lease_seconds: u32) -> Result<String, RunPodError> {
    time::OffsetDateTime::now_utc()
        .checked_add(time::Duration::seconds(i64::from(lease_seconds)))
        .ok_or(RunPodError::InvalidTarget)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| RunPodError::InvalidTarget)
}
fn valid_provider_id(value: &str) -> bool {
    let valid_byte = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_');
    !value.is_empty() && value.len() <= 191 && value.bytes().all(valid_byte)
}
fn valid_immutable_worker_image(value: &str) -> bool {
    valid_worker_image(value)
        && value
            .rsplit_once("@sha256:")
            .is_some_and(|(_, digest)| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
fn deserialize_hourly_cost<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let encoded = match value {
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => value,
        _ => return Err(de::Error::custom("hourly cost must be a decimal")),
    };
    decimal_micros(&encoded)
        .ok_or_else(|| de::Error::custom("hourly cost must be a non-negative decimal"))
        .map(Some)
}
fn decimal_micros(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    (!whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(())?;
    let kept = &fraction[..fraction.len().min(6)];
    let micros = format!("{whole}{kept:0<6}").parse::<u64>().ok()?;
    let round_up = fraction
        .as_bytes()
        .get(6..)
        .is_some_and(|tail| tail.iter().any(|byte| *byte != b'0'));
    micros.checked_add(u64::from(round_up))
}
