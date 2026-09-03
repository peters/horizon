//! Azure Container Apps Jobs lifecycle adapter for durable cloud work.
use super::{CloudJobId, CloudProvider, CloudWorkflowId, WorkerTarget, validation::valid_worker_image};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
mod http;
mod model;
use model::{ApiExecution, ApiJob, CreateJobRequest, create_request, status_from_resource, worker_from_job};
#[cfg(test)]
mod tests;
const OWNER_TAG: &str = "horizon-owned-by";
const WORKFLOW_TAG: &str = "horizon-workflow-id";
const JOB_TAG: &str = "horizon-job-id";
const PROTOCOL_TAG: &str = "horizon-protocol-version";
const PROFILE_TAG: &str = "horizon-profile";
const DEADLINE_TAG: &str = "horizon-delete-after";
const COST_TAG: &str = "horizon-hourly-cost-micros";
const DISK_TAG: &str = "horizon-disk-gib";
const WORKFLOW_ENV: &str = "HORIZON_WORKFLOW_ID";
const JOB_ENV: &str = "HORIZON_JOB_ID";
const PROTOCOL_ENV: &str = "HORIZON_CLOUD_PROTOCOL_VERSION";
const TERMINATE_ENV: &str = "HORIZON_TERMINATE_AFTER";
const MIN_LEASE_SECONDS: u32 = 300;
const MAX_LEASE_SECONDS: u32 = 30 * 24 * 60 * 60;
const LEASE_CLOCK_SKEW_SECONDS: i64 = 120;
const CONSUMPTION_PROFILE: &str = "Consumption";
const JOB_POLL_BACKOFF_MS: [u64; 8] = [0, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000];
#[derive(Clone)]
pub struct AzureAccessToken(String);
impl AzureAccessToken {
    /// # Errors
    /// Rejects empty, oversized, whitespace-containing, or non-ASCII tokens.
    pub fn new(value: impl Into<String>) -> Result<Self, AzureError> {
        let value = value.into();
        let valid = valid_text(&value, 16_384) && value.bytes().all(|byte| !byte.is_ascii_whitespace());
        valid.then_some(Self(value)).ok_or(AzureError::InvalidAccessToken)
    }
    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for AzureAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AzureAccessToken(<redacted>)")
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureProfile {
    pub name: String,
    pub subscription_id: String,
    pub resource_group: String,
    pub environment_id: String,
    pub location: String,
    pub cpu_millicores: u32,
    pub memory_mib: u32,
    pub hourly_cost_micros: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureWorker {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub resource_id: String,
    pub name: String,
    pub target: WorkerTarget,
    pub profile: AzureProfile,
    pub delete_after: String,
}
impl AzureWorker {
    /// # Errors
    /// Rejects malformed or internally inconsistent worker identities.
    pub fn validate(&self) -> Result<(), AzureError> {
        let name = resource_name(self.workflow_id, self.job_id);
        let expected = resource_id(&self.profile.subscription_id, &self.profile.resource_group, &self.name);
        let valid = self.name == name
            && validate_target(&self.target, &self.profile).is_ok()
            && valid_deadline(&self.delete_after)
            && self.resource_id.eq_ignore_ascii_case(&expected);
        valid.then_some(()).ok_or(AzureError::InvalidPersistedWorker)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AzureLifecycle {
    Provisioning,
    Running,
    Succeeded,
    Failed,
    Stopped,
    Unknown,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AzureWorkerStatus {
    pub worker: AzureWorker,
    pub lifecycle: AzureLifecycle,
    pub execution_id: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AzureEnsure {
    Created(AzureWorkerStatus),
    Reused(AzureWorkerStatus),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AzureCleanup {
    Deleted,
    DeletionPending,
    AlreadyAbsent,
}
pub struct AzureClient {
    profile: AzureProfile,
    transport: Box<dyn Transport>,
}
impl AzureClient {
    /// # Errors
    /// Rejects an invalid profile or control-plane URL.
    pub fn new(token: &AzureAccessToken, profile: AzureProfile) -> Result<Self, AzureError> {
        validate_profile(&profile)?;
        let transport = Box::new(http::AzureHttp::new(token, &profile));
        Ok(Self { profile, transport })
    }
    /// # Errors
    /// Fails closed on invalid configuration, identity drift, duplicate executions, provider errors, or price.
    pub fn ensure_worker(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
    ) -> Result<AzureEnsure, AzureError> {
        validate_target(target, &self.profile)?;
        let name = resource_name(workflow_id, job_id);
        let (mut job, created) = if let Some(job) = self.transport.get(&name)? {
            (job, false)
        } else {
            enforce_cost(target, self.profile.hourly_cost_micros)?;
            let request = create_request(workflow_id, job_id, target, &self.profile)?;
            let created = self.transport.create(&name, &request)?;
            (created.job, created.created)
        };
        let worker = match worker_from_job(&job, workflow_id, job_id, target, &self.profile) {
            Ok(worker) => worker,
            Err(error) if created => return self.cleanup_creation_failure(&name, None, error),
            Err(error) => return Err(error),
        };
        if !recovered_deadline_valid(&worker.delete_after, target.lease_seconds) {
            return self.cleanup_creation_failure(&name, Some(&worker), AzureError::ResourceIdentityMismatch);
        }
        if created && !job.ready_to_start() {
            job = match self.await_created_job(&worker) {
                Ok(job) => job,
                Err(error) => return self.cleanup_creation_failure(&name, Some(&worker), error),
            };
        }
        let result = (|| {
            enforce_cost(target, worker.profile.hourly_cost_micros)?;
            let existing = self.execution_for(&job, &worker.resource_id)?;
            // The persisted job is the at-most-once fence; retries only reconcile its execution.
            let execution = if created && existing.is_none() && job.ready_to_start() {
                status_from_resource(&job, &worker, None)?;
                self.transport.start(&name)?
            } else {
                existing
            };
            status_from_resource(&job, &worker, execution.as_ref())
        })();
        let status = match result {
            Ok(status) => status,
            Err(error) if created || matches!(&error, AzureError::HourlyCostRejected { .. }) => {
                return self.cleanup_creation_failure(&name, Some(&worker), error);
            }
            Err(error) => return Err(error),
        };
        Ok(if created {
            AzureEnsure::Created(status)
        } else {
            AzureEnsure::Reused(status)
        })
    }
    /// # Errors
    /// Fails closed if the persisted or provider identity has drifted.
    pub fn inspect_worker(&self, worker: &AzureWorker) -> Result<Option<AzureWorkerStatus>, AzureError> {
        self.validate_scope(worker)?;
        let Some(job) = self.transport.get(&worker.name)? else {
            return Ok(None);
        };
        let execution = self.execution_for(&job, &worker.resource_id)?;
        status_from_resource(&job, worker, execution.as_ref()).map(Some)
    }
    /// # Errors
    /// Refuses deletion when persisted or provider identity differs.
    pub fn delete_worker(&self, worker: &AzureWorker) -> Result<AzureCleanup, AzureError> {
        self.validate_scope(worker)?;
        let Some(job) = self.transport.get(&worker.name)? else {
            return Ok(AzureCleanup::AlreadyAbsent);
        };
        status_from_resource(&job, worker, None)?;
        self.transport.delete(&worker.name)
    }
    fn validate_scope(&self, worker: &AzureWorker) -> Result<(), AzureError> {
        worker.validate()?;
        let profile = &self.profile;
        let expected = resource_id(&profile.subscription_id, &profile.resource_group, &worker.name);
        let valid = worker.resource_id.eq_ignore_ascii_case(&expected);
        valid.then_some(()).ok_or(AzureError::InvalidPersistedWorker)
    }
    fn single_execution(&self, name: &str, resource_id: &str) -> Result<Option<ApiExecution>, AzureError> {
        let executions = self.transport.executions(name)?;
        if executions.len() > 1 {
            return Err(AzureError::AmbiguousExecutions {
                resource_id: resource_id.to_string(),
                count: executions.len(),
            });
        }
        Ok(executions.into_iter().next())
    }
    fn execution_for(&self, job: &ApiJob, resource_id: &str) -> Result<Option<ApiExecution>, AzureError> {
        if !job.ready_to_start() {
            return Ok(None);
        }
        self.single_execution(&job.name, resource_id)
    }
    fn await_created_job(&self, worker: &AzureWorker) -> Result<ApiJob, AzureError> {
        await_job(
            || self.transport.get(&worker.name),
            |job| {
                status_from_resource(job, worker, None)?;
                Ok(job.ready_to_start())
            },
            "job provisioning",
        )
    }
    fn cleanup_creation_failure<T>(
        &self,
        name: &str,
        worker: Option<&AzureWorker>,
        error: AzureError,
    ) -> Result<T, AzureError> {
        let cleanup_failed = || AzureError::CleanupFailed {
            resource_id: resource_id(&self.profile.subscription_id, &self.profile.resource_group, name),
        };
        let Some(worker) = worker else {
            return Err(cleanup_failed());
        };
        let job = match self.transport.get(name) {
            Ok(Some(job)) => job,
            Ok(None) => return Err(error),
            Err(_) => return Err(cleanup_failed()),
        };
        if status_from_resource(&job, worker, None).is_err() || self.transport.delete(name).is_err() {
            return Err(cleanup_failed());
        }
        for delay_millis in JOB_POLL_BACKOFF_MS {
            std::thread::sleep(std::time::Duration::from_millis(delay_millis));
            if matches!(self.transport.get(name), Ok(None)) {
                return Err(error);
            }
        }
        Err(cleanup_failed())
    }
    #[cfg(test)]
    fn with_transport(profile: AzureProfile, transport: impl Transport + 'static) -> Self {
        let transport = Box::new(transport);
        Self { profile, transport }
    }
}
fn await_job(
    mut inspect: impl FnMut() -> Result<Option<ApiJob>, AzureError>,
    mut ready: impl FnMut(&ApiJob) -> Result<bool, AzureError>,
    operation: &'static str,
) -> Result<ApiJob, AzureError> {
    for delay_millis in JOB_POLL_BACKOFF_MS {
        std::thread::sleep(std::time::Duration::from_millis(delay_millis));
        if let Some(job) = inspect()?
            && ready(&job)?
        {
            return Ok(job);
        }
    }
    Err(AzureError::RequestFailed { operation })
}
#[derive(Debug, Error, Eq, PartialEq)]
pub enum AzureError {
    #[error("Azure access token is invalid")]
    InvalidAccessToken,
    #[error("Azure worker profile is invalid")]
    InvalidProfile,
    #[error("Azure worker target is invalid")]
    InvalidTarget,
    #[error("persisted Azure worker identity is invalid")]
    InvalidPersistedWorker,
    #[error("Azure returned an invalid or mismatched resource identity")]
    ResourceIdentityMismatch,
    #[error("Azure returned {count} executions for {resource_id}")]
    AmbiguousExecutions { resource_id: String, count: usize },
    #[error("Azure request failed during {operation}")]
    RequestFailed { operation: &'static str },
    #[error("Azure returned HTTP {status} during {operation}")]
    UnexpectedStatus { operation: &'static str, status: u16 },
    #[error("Azure returned a malformed response during {operation}")]
    InvalidResponse { operation: &'static str },
    #[error("Azure worker operation failed and exact cleanup also failed for {resource_id}")]
    CleanupFailed { resource_id: String },
    #[error("Azure hourly cost {actual} exceeded configured maximum {maximum}")]
    HourlyCostRejected { actual: u64, maximum: u64 },
}
struct CreateResult {
    job: ApiJob,
    created: bool,
}
trait Transport: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<ApiJob>, AzureError>;
    fn create(&self, name: &str, request: &CreateJobRequest) -> Result<CreateResult, AzureError>;
    fn executions(&self, name: &str) -> Result<Vec<ApiExecution>, AzureError>;
    fn start(&self, name: &str) -> Result<Option<ApiExecution>, AzureError>;
    fn delete(&self, name: &str) -> Result<AzureCleanup, AzureError>;
}
fn validate_profile(profile: &AzureProfile) -> Result<(), AzureError> {
    let prefix = format!("/subscriptions/{}/", profile.subscription_id);
    let valid = valid_text(&profile.name, 191)
        && uuid::Uuid::parse_str(&profile.subscription_id).is_ok()
        && valid_resource_group(&profile.resource_group)
        && valid_arm_id(&profile.environment_id)
        && profile
            .environment_id
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        && valid_text(&profile.location, 90)
        && profile.location.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && (250..=4_000).contains(&profile.cpu_millicores)
        && profile.cpu_millicores.is_multiple_of(250)
        && profile.memory_mib == profile.cpu_millicores / 250 * 512
        && profile.hourly_cost_micros > 0;
    valid.then_some(()).ok_or(AzureError::InvalidProfile)
}
fn validate_target(target: &WorkerTarget, profile: &AzureProfile) -> Result<(), AzureError> {
    validate_profile(profile)?;
    let valid = target.provider == CloudProvider::Azure
        && target.profile == profile.name
        && (1..=ephemeral_disk_limit(profile.cpu_millicores)).contains(&target.disk_gib)
        && (MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&target.lease_seconds)
        && target.max_hourly_cost_micros.is_some_and(|maximum| maximum > 0)
        && valid_immutable_image(&target.image);
    valid.then_some(()).ok_or(AzureError::InvalidTarget)
}
fn enforce_cost(target: &WorkerTarget, actual: u64) -> Result<(), AzureError> {
    let maximum = target.max_hourly_cost_micros.ok_or(AzureError::InvalidTarget)?;
    if actual <= maximum {
        Ok(())
    } else {
        Err(AzureError::HourlyCostRejected { actual, maximum })
    }
}
fn resource_name(workflow_id: CloudWorkflowId, job_id: CloudJobId) -> String {
    let workflow = workflow_id.to_string().replace('-', "");
    let job = job_id.to_string().replace('-', "");
    format!("hz-{}-{}", &workflow[..12], &job[..12])
}
fn resource_id(subscription: &str, resource_group: &str, name: &str) -> String {
    format!("/subscriptions/{subscription}/resourceGroups/{resource_group}/providers/Microsoft.App/jobs/{name}")
}
fn deletion_deadline(lease_seconds: u32) -> Result<String, AzureError> {
    time::OffsetDateTime::now_utc()
        .checked_add(time::Duration::seconds(i64::from(lease_seconds)))
        .ok_or(AzureError::InvalidTarget)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| AzureError::InvalidTarget)
}
fn recovered_deadline_valid(value: &str, lease_seconds: u32) -> bool {
    let Ok(deadline) = time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339) else {
        return false;
    };
    let now = time::OffsetDateTime::now_utc();
    deadline >= now - time::Duration::seconds(LEASE_CLOCK_SKEW_SECONDS)
        && deadline <= now + time::Duration::seconds(i64::from(lease_seconds) + LEASE_CLOCK_SKEW_SECONDS)
}
fn ephemeral_disk_limit(cpu_millicores: u32) -> u32 {
    (cpu_millicores / 250).checked_next_power_of_two().unwrap_or(8).min(8)
}
fn valid_immutable_image(value: &str) -> bool {
    valid_worker_image(value)
        && value
            .rsplit_once("@sha256:")
            .is_some_and(|(_, digest)| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
fn valid_deadline(value: &str) -> bool {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok()
}
fn valid_resource_group(value: &str) -> bool {
    valid_text(value, 90)
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-()".contains(&byte))
}
fn valid_arm_id(value: &str) -> bool {
    value.starts_with("/subscriptions/")
        && valid_text(value, 2_048)
        && !value.contains(['?', '#'])
        && value.bytes().all(|byte| !byte.is_ascii_whitespace())
}
fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && value.is_ascii()
        && !value.chars().any(char::is_control)
}
