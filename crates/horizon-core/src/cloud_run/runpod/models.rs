use super::{CloudJobId, CloudWorkflowId, resource_name, valid_immutable_worker_image, valid_provider_id};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPodProfile {
    pub name: String,
    pub gpu_type_ids: Vec<String>,
    pub gpu_count: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_cuda_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_center_id: Option<String>,
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
    pub fn validate(&self) -> Result<(), RunPodError> {
        let valid = valid_provider_id(&self.pod_id)
            && self.name == resource_name(self.workflow_id, self.job_id)
            && valid_immutable_worker_image(&self.image)
            && time::OffsetDateTime::parse(&self.terminate_after, &time::format_description::well_known::Rfc3339)
                .is_ok();
        valid.then_some(()).ok_or(RunPodError::InvalidPersistedWorker)
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
pub struct RunPodSshEndpoint {
    pub username: String,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPodWorkerStatus {
    pub worker: RunPodWorker,
    pub lifecycle: RunPodLifecycle,
    pub ssh_username: Option<String>,
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
    #[error("RunPod creation for {name} was already claimed but no pod became visible")]
    CreationUnresolved { name: String },
    #[error("RunPod durable creation fence failed: {reason}")]
    CreationFenceFailed { reason: String },
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
    #[error("RunPod pod {pod_id} deletion could not be verified")]
    DeletionVerificationFailed { pod_id: String },
    #[error("RunPod recovered worker lease was outside the requested bound and was deleted")]
    LeaseDeadlineRejected { worker: Box<RunPodWorker> },
    #[error("RunPod recovered worker lease was outside the requested bound but cleanup failed")]
    LeaseRejectionCleanupFailed { worker: Box<RunPodWorker> },
    #[error("RunPod hourly cost was rejected and cleanup was requested")]
    HourlyCostRejected {
        worker: Box<RunPodWorker>,
        actual: Option<u64>,
        maximum: u64,
    },
    #[error("RunPod hourly cost was rejected but exact-resource cleanup failed")]
    CostRejectionCleanupFailed { worker: Box<RunPodWorker> },
}
