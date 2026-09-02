//! `RunPod` Secure Cloud lifecycle adapter for durable cloud workers.

use super::{CloudJobId, CloudProvider, CloudWorkflowId, WorkerTarget, validation::valid_worker_image};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeMap, env, fmt};
use thiserror::Error;

mod http;
#[cfg(test)]
mod tests;
const WORKFLOW_ENV: &str = "HORIZON_WORKFLOW_ID";
const JOB_ENV: &str = "HORIZON_JOB_ID";
const PROTOCOL_ENV: &str = "HORIZON_CLOUD_PROTOCOL_VERSION";
const TERMINATE_ENV: &str = "HORIZON_TERMINATE_AFTER";
const MIN_LEASE_SECONDS: u32 = 300;
const MAX_LEASE_SECONDS: u32 = 30 * 24 * 60 * 60;

/// Secret `RunPod` bearer token. Debug output is always redacted.
#[derive(Clone)]
pub struct RunPodApiKey(String);
impl RunPodApiKey {
    /// Validate a token supplied by a secret store.
    ///
    /// # Errors
    /// Rejects empty, oversized, whitespace-containing, or non-ASCII tokens.
    pub fn new(value: impl Into<String>) -> Result<Self, RunPodError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 4_096
            && value.is_ascii()
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_whitespace() && !byte.is_ascii_control());
        valid.then_some(Self(value)).ok_or(RunPodError::InvalidApiKey)
    }
    /// Load the token injected by the control plane.
    ///
    /// # Errors
    /// Returns an error when `RUNPOD_API_KEY` is missing, non-Unicode, or invalid.
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

/// Non-secret `RunPod` placement and connectivity settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPodProfile {
    pub name: String,
    pub gpu_type_ids: Vec<String>,
    pub gpu_count: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_cuda_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_center_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    pub volume_gib: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_download_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_upload_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_disk_bandwidth_mbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_registry_auth_id: Option<String>,
}
/// Persistable provider resource identity used for exact cleanup after restart.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPodWorker {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub pod_id: String,
    pub name: String,
    pub image: String,
    pub terminate_after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hourly_cost_micros: Option<u64>,
}

impl RunPodWorker {
    /// Validate persisted identity before any provider mutation.
    /// # Errors
    /// Rejects malformed IDs, names, images, or cost values.
    pub fn validate(&self) -> Result<(), RunPodError> {
        let expected_name = resource_name(self.workflow_id, self.job_id);
        if !valid_provider_id(&self.pod_id)
            || self.name != expected_name
            || !valid_immutable_worker_image(&self.image)
            || time::OffsetDateTime::parse(&self.terminate_after, &time::format_description::well_known::Rfc3339)
                .is_err()
        {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunPodLifecycle {
    Provisioning,
    Running,
    Exited,
    Failed,
    Terminated,
    Unknown,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPodWorkerStatus {
    pub worker: RunPodWorker,
    pub lifecycle: RunPodLifecycle,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunPodEnsure {
    Created(RunPodWorkerStatus),
    Reused(RunPodWorkerStatus),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunPodCleanup {
    Deleted,
    AlreadyAbsent,
}
/// Blocking `RunPod` client. Call it from a worker thread when used by an async UI.
pub struct RunPodClient {
    transport: Box<dyn Transport>,
}
impl RunPodClient {
    /// Build a production client pinned to `RunPod`'s HTTPS control planes.
    /// # Errors
    /// Returns an error if the fixed control-plane URL cannot be initialized.
    pub fn new(api_key: &RunPodApiKey) -> Result<Self, RunPodError> {
        Ok(Self {
            transport: Box::new(http::RunPodHttp::new(api_key)?),
        })
    }

    /// Create a worker once, or adopt the single exact resource from an interrupted attempt.
    /// # Errors
    /// Fails closed on invalid configuration, ambiguous resources, identity mismatch,
    /// provider errors, missing cost data under a cost cap, or a breached cost cap.
    pub fn ensure_worker(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
    ) -> Result<RunPodEnsure, RunPodError> {
        validate_target(target, profile)?;
        let name = resource_name(workflow_id, job_id);
        let matches = self.transport.list_by_name(&name)?;
        let (pod, created, expected_deadline) = match matches.as_slice() {
            [] => {
                let request = CreatePodRequest::new(workflow_id, job_id, target, profile, name.clone())?;
                let expected_deadline = request.terminate_after.clone();
                (self.transport.create(&request)?, true, Some(expected_deadline))
            }
            [pod] => (pod.clone(), false, None),
            _ => {
                return Err(RunPodError::AmbiguousResource {
                    name,
                    count: matches.len(),
                });
            }
        };
        let status = match status_from_pod(&pod, workflow_id, job_id, target, expected_deadline.as_deref()) {
            Ok(status) => status,
            Err(error) if created => {
                if self.transport.delete(&pod.id).is_err() {
                    return Err(RunPodError::CreationCleanupFailed { pod_id: pod.id });
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        self.enforce_cost_limit(&status.worker, target.max_hourly_cost_micros)?;
        Ok(if created {
            RunPodEnsure::Created(status)
        } else {
            RunPodEnsure::Reused(status)
        })
    }

    /// Inspect an exact persisted worker, returning `None` after provider deletion.
    /// # Errors
    /// Fails if persisted or provider identity differs from the expected workflow resource.
    pub fn inspect_worker(&self, worker: &RunPodWorker) -> Result<Option<RunPodWorkerStatus>, RunPodError> {
        worker.validate()?;
        self.transport
            .get(&worker.pod_id)?
            .map(|pod| status_from_resource(&pod, worker))
            .transpose()
    }
    /// Delete only the exact resource proven to belong to the persisted workflow and job.
    /// # Errors
    /// Fails closed on identity mismatch or provider errors without issuing a delete.
    pub fn delete_worker(&self, worker: &RunPodWorker) -> Result<RunPodCleanup, RunPodError> {
        worker.validate()?;
        let Some(pod) = self.transport.get(&worker.pod_id)? else {
            return Ok(RunPodCleanup::AlreadyAbsent);
        };
        status_from_resource(&pod, worker)?;
        self.transport.delete(&worker.pod_id)
    }

    fn enforce_cost_limit(&self, worker: &RunPodWorker, maximum: Option<u64>) -> Result<(), RunPodError> {
        let Some(maximum) = maximum else {
            return Ok(());
        };
        let Some(actual) = worker.hourly_cost_micros else {
            return self.cleanup_after_cost_rejection(worker, None, maximum);
        };
        if actual > maximum {
            return self.cleanup_after_cost_rejection(worker, Some(actual), maximum);
        }
        Ok(())
    }

    fn cleanup_after_cost_rejection(
        &self,
        worker: &RunPodWorker,
        actual: Option<u64>,
        maximum: u64,
    ) -> Result<(), RunPodError> {
        if self.delete_worker(worker).is_err() {
            return Err(RunPodError::CostRejectionCleanupFailed {
                worker: Box::new(worker.clone()),
            });
        }
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
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RunPodError {
    #[error("RUNPOD_API_KEY is not available")]
    MissingApiKey,
    #[error("RunPod API key is invalid")]
    InvalidApiKey,
    #[error("RunPod worker target or profile is invalid")]
    InvalidTarget,
    #[error("persisted RunPod worker identity is invalid")]
    InvalidPersistedWorker,
    #[error("RunPod returned an invalid or mismatched resource identity")]
    ResourceIdentityMismatch,
    #[error("RunPod returned {count} resources for deterministic name {name}")]
    AmbiguousResource { name: String, count: usize },
    #[error("RunPod request failed during {operation}")]
    RequestFailed { operation: &'static str },
    #[error("RunPod returned HTTP {status} during {operation}")]
    UnexpectedStatus { operation: &'static str, status: u16 },
    #[error("RunPod returned a malformed response during {operation}")]
    InvalidResponse { operation: &'static str },
    #[error("RunPod has no matching GPU capacity")]
    CapacityUnavailable,
    #[error("RunPod pod {pod_id} could not be verified after creation and was deleted")]
    CreationVerificationFailed { pod_id: String },
    #[error("RunPod pod {pod_id} could not be verified or deleted after creation")]
    CreationCleanupFailed { pod_id: String },
    #[error("RunPod hourly cost was rejected and cleanup was requested")]
    HourlyCostRejected {
        worker: Box<RunPodWorker>,
        actual: Option<u64>,
        maximum: u64,
    },
    #[error("RunPod hourly cost was rejected but exact-resource cleanup failed")]
    CostRejectionCleanupFailed { worker: Box<RunPodWorker> },
}

trait Transport: Send + Sync {
    fn list_by_name(&self, name: &str) -> Result<Vec<ApiPod>, RunPodError>;
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError>;
    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError>;
    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatePodRequest {
    allowed_cuda_versions: Vec<String>,
    cloud_type: &'static str,
    container_disk_in_gb: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    container_registry_auth_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    data_center_ids: Vec<String>,
    env: Vec<CreatePodEnv>,
    gpu_count: u16,
    #[serde(rename = "gpuTypeIdList")]
    gpu_type_ids: Vec<String>,
    image_name: String,
    #[serde(rename = "minDisk", skip_serializing_if = "Option::is_none")]
    min_disk_bandwidth_mbps: Option<u32>,
    #[serde(rename = "minDownload", skip_serializing_if = "Option::is_none")]
    min_download_mbps: Option<u32>,
    #[serde(rename = "minUpload", skip_serializing_if = "Option::is_none")]
    min_upload_mbps: Option<u32>,
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    ports: String,
    start_ssh: bool,
    support_public_ip: bool,
    terminate_after: String,
    volume_in_gb: u32,
    volume_mount_path: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct CreatePodEnv {
    key: String,
    value: String,
}

impl CreatePodRequest {
    fn new(
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        profile: &RunPodProfile,
        name: String,
    ) -> Result<Self, RunPodError> {
        let terminate_after = termination_deadline(target.lease_seconds)?;
        let env = [
            (WORKFLOW_ENV, workflow_id.to_string()),
            (JOB_ENV, job_id.to_string()),
            (PROTOCOL_ENV, super::CLOUD_RUN_PROTOCOL_VERSION.to_string()),
            (TERMINATE_ENV, terminate_after.clone()),
        ]
        .into_iter()
        .map(|(key, value)| CreatePodEnv {
            key: key.to_string(),
            value,
        })
        .collect();
        Ok(Self {
            allowed_cuda_versions: profile.allowed_cuda_versions.clone(),
            cloud_type: "SECURE",
            container_disk_in_gb: target.disk_gib,
            container_registry_auth_id: profile.container_registry_auth_id.clone(),
            data_center_ids: profile.data_center_ids.clone(),
            env,
            gpu_count: profile.gpu_count,
            gpu_type_ids: profile.gpu_type_ids.clone(),
            image_name: target.image.clone(),
            min_disk_bandwidth_mbps: profile.min_disk_bandwidth_mbps,
            min_download_mbps: profile.min_download_mbps,
            min_upload_mbps: profile.min_upload_mbps,
            name,
            ports: profile.ports.join(","),
            start_ssh: true,
            support_public_ip: true,
            terminate_after,
            volume_in_gb: profile.volume_gib,
            volume_mount_path: "/workspace",
        })
    }
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
impl ApiPod {
    fn hourly_cost_micros(&self) -> Option<u64> {
        self.cost
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ApiSsh {
    direct: Option<ApiSshEndpoint>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiSshEndpoint {
    host: String,
    port: u16,
}
fn validate_target(target: &WorkerTarget, profile: &RunPodProfile) -> Result<(), RunPodError> {
    let safe_text = |value: &str| {
        !value.is_empty() && value.len() <= 191 && value.trim() == value && !value.chars().any(char::is_control)
    };
    let safe_id = |value: &str| {
        safe_text(value)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    let safe_port = |value: &str| {
        value.split_once('/').is_some_and(|(port, protocol)| {
            port.parse::<u16>().is_ok_and(|port| port > 0) && matches!(protocol, "tcp" | "http")
        })
    };
    let nonzero = |value: Option<u32>| value.is_none_or(|value| value > 0);
    let valid = target.provider == CloudProvider::RunPod
        && target.profile == profile.name
        && target.disk_gib > 0
        && (MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&target.lease_seconds)
        && target.max_hourly_cost_micros != Some(0)
        && valid_immutable_worker_image(&target.image)
        && safe_text(&profile.name)
        && !profile.gpu_type_ids.is_empty()
        && profile.gpu_type_ids.iter().all(|value| safe_text(value))
        && profile.gpu_count > 0
        && profile.allowed_cuda_versions.iter().all(|value| {
            value.split_once('.').is_some_and(|(major, minor)| {
                !major.is_empty()
                    && !minor.is_empty()
                    && major.bytes().all(|byte| byte.is_ascii_digit())
                    && minor.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        && profile.data_center_ids.iter().all(|value| safe_id(value))
        && profile.ports.iter().all(|value| safe_port(value))
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
) -> Result<RunPodWorkerStatus, RunPodError> {
    let terminate_after = pod
        .env
        .get(TERMINATE_ENV)
        .cloned()
        .ok_or(RunPodError::ResourceIdentityMismatch)?;
    if expected_deadline.is_some_and(|expected| expected != terminate_after) {
        return Err(RunPodError::ResourceIdentityMismatch);
    }
    let worker = RunPodWorker {
        workflow_id,
        job_id,
        pod_id: pod.id.clone(),
        name: resource_name(workflow_id, job_id),
        image: target.image.clone(),
        terminate_after,
        hourly_cost_micros: pod.hourly_cost_micros(),
    };
    status_from_resource(pod, &worker)
}

fn status_from_resource(pod: &ApiPod, worker: &RunPodWorker) -> Result<RunPodWorkerStatus, RunPodError> {
    worker.validate()?;
    let owned = pod.id == worker.pod_id
        && pod.name == worker.name
        && pod.image == worker.image
        && pod.env.get(WORKFLOW_ENV) == Some(&worker.workflow_id.to_string())
        && pod.env.get(JOB_ENV) == Some(&worker.job_id.to_string())
        && pod.env.get(PROTOCOL_ENV) == Some(&super::CLOUD_RUN_PROTOCOL_VERSION.to_string());
    let owned = owned && pod.env.get(TERMINATE_ENV) == Some(&worker.terminate_after);
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
            hourly_cost_micros: pod.hourly_cost_micros().or(worker.hourly_cost_micros),
            ..worker.clone()
        },
        lifecycle,
        ssh_host: direct_ssh.map(|endpoint| endpoint.host.clone()),
        ssh_port: direct_ssh.map(|endpoint| endpoint.port),
    })
}

fn resource_name(workflow_id: CloudWorkflowId, job_id: CloudJobId) -> String {
    format!("horizon-{workflow_id}-{job_id}")
}

fn termination_deadline(lease_seconds: u32) -> Result<String, RunPodError> {
    time::OffsetDateTime::now_utc()
        .checked_add(time::Duration::seconds(i64::from(lease_seconds)))
        .ok_or(RunPodError::InvalidTarget)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| RunPodError::InvalidTarget)
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 191
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_immutable_worker_image(value: &str) -> bool {
    valid_worker_image(value) && value.rsplit_once("@sha256:").is_some()
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
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<u64>().ok()?.checked_mul(1_000_000)?;
    let kept = &fraction[..fraction.len().min(6)];
    let mut fraction_micros = if kept.is_empty() {
        0
    } else {
        kept.parse::<u64>().ok()? * 10_u64.pow(u32::try_from(6 - kept.len()).ok()?)
    };
    if fraction
        .as_bytes()
        .get(6..)
        .is_some_and(|tail| tail.iter().any(|byte| *byte != b'0'))
    {
        fraction_micros = fraction_micros.checked_add(1)?;
    }
    whole.checked_add(fraction_micros)
}
