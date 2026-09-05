use super::super::interactive_worker::{InteractiveWorkerLease, InteractiveWorkerLifetime};
use super::{CloudJobId, CloudWorkflowId, resource_name, valid_immutable_worker_image, valid_provider_id};
use serde::{Deserialize, Deserializer, Serialize};
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
#[serde(try_from = "WorkerSnapshot", into = "WorkerSnapshot")]
pub struct RunPodWorker {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub pod_id: String,
    pub name: String,
    pub image: String,
    pub lifetime: InteractiveWorkerLifetime,
    pub hourly_cost_micros: Option<u64>,
}
impl RunPodWorker {
    /// Validate persisted identity before any provider mutation.
    /// # Errors
    pub fn validate(&self) -> Result<(), RunPodError> {
        let valid = valid_provider_id(&self.pod_id)
            && self.name == resource_name(self.workflow_id, self.job_id)
            && valid_immutable_worker_image(&self.image)
            && self.lifetime.has_valid_shape();
        valid.then_some(()).ok_or(RunPodError::InvalidPersistedWorker)
    }
}

/// Keep the legacy flat timestamp representation; omission never grants persistence.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerSnapshot {
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    pod_id: String,
    name: String,
    image: String,
    #[serde(default, deserialize_with = "present_value", skip_serializing_if = "Option::is_none")]
    terminate_after: Option<String>,
    #[serde(default, deserialize_with = "present_value", skip_serializing_if = "Option::is_none")]
    lifetime: Option<PersistentMarker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hourly_cost_micros: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistentMarker {
    Persistent,
}

fn present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<WorkerSnapshot> for RunPodWorker {
    type Error = RunPodError;

    fn try_from(value: WorkerSnapshot) -> Result<Self, Self::Error> {
        let lifetime = match (value.terminate_after, value.lifetime) {
            (Some(terminate_after), None) => {
                InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease { terminate_after })
            }
            (None, Some(PersistentMarker::Persistent)) => InteractiveWorkerLifetime::Persistent,
            _ => return Err(RunPodError::InvalidPersistedWorker),
        };
        let worker = Self {
            workflow_id: value.workflow_id,
            job_id: value.job_id,
            pod_id: value.pod_id,
            name: value.name,
            image: value.image,
            lifetime,
            hourly_cost_micros: value.hourly_cost_micros,
        };
        worker.validate()?;
        Ok(worker)
    }
}

impl From<RunPodWorker> for WorkerSnapshot {
    fn from(value: RunPodWorker) -> Self {
        let (terminate_after, lifetime) = match value.lifetime {
            InteractiveWorkerLifetime::TimeLimited(lease) => (Some(lease.terminate_after), None),
            InteractiveWorkerLifetime::Persistent => (None, Some(PersistentMarker::Persistent)),
        };
        Self {
            workflow_id: value.workflow_id,
            job_id: value.job_id,
            pod_id: value.pod_id,
            name: value.name,
            image: value.image,
            terminate_after,
            lifetime,
            hourly_cost_micros: value.hourly_cost_micros,
        }
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
    #[error("persistent worker {name} creation is uncertain; reconcile before any retry: {cause}")]
    PersistentCreationUnresolved {
        name: String,
        #[source]
        cause: Box<RunPodError>,
    },
    #[error(
        "persistent worker {name} (pod {pod_id}) could not be verified; it was not deleted and may remain billable"
    )]
    PersistentCreationReconciliationRequired { name: String, pod_id: String },
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
    #[error("persistent worker hourly cost was rejected; it was not deleted and may remain billable")]
    PersistentWorkerCostRejected {
        worker: Box<RunPodWorker>,
        actual: Option<u64>,
        maximum: u64,
    },
}
