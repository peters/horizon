//! Azure Container Apps Jobs lifecycle adapter for durable cloud work.
use super::{CloudJobId, CloudProvider, CloudWorkflowId, WorkerTarget, validation::valid_worker_image};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
use thiserror::Error;
mod http;
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
#[derive(Clone)]
pub struct AzureAccessToken(String);
impl AzureAccessToken {
    /// # Errors
    /// Rejects empty, oversized, whitespace-containing, or non-ASCII tokens.
    pub fn new(value: impl Into<String>) -> Result<Self, AzureError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 16_384
            && value.is_ascii()
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_whitespace() && !byte.is_ascii_control());
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
pub struct AzureRegistry {
    pub server: String,
    pub identity_id: String,
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
    pub ephemeral_disk_gib: u32,
    pub hourly_cost_micros: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<AzureRegistry>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureWorker {
    pub workflow_id: CloudWorkflowId,
    pub job_id: CloudJobId,
    pub resource_id: String,
    pub name: String,
    pub profile: String,
    pub environment_id: String,
    pub image: String,
    pub disk_gib: u32,
    pub lease_seconds: u32,
    pub delete_after: String,
    pub hourly_cost_micros: u64,
}
impl AzureWorker {
    /// # Errors
    /// Rejects malformed or internally inconsistent worker identities.
    pub fn validate(&self) -> Result<(), AzureError> {
        let name = resource_name(self.workflow_id, self.job_id);
        let valid = self.name == name
            && valid_arm_id(&self.resource_id)
            && self
                .resource_id
                .to_ascii_lowercase()
                .ends_with(&format!("/providers/microsoft.app/jobs/{name}"))
            && valid_text(&self.profile, 191)
            && valid_arm_id(&self.environment_id)
            && valid_immutable_image(&self.image)
            && self.disk_gib > 0
            && (MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&self.lease_seconds)
            && valid_deadline(&self.delete_after)
            && self.hourly_cost_micros > 0;
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
    /// Fails closed on invalid configuration, identity drift, duplicate executions,
    /// provider errors, or an exceeded hourly price.
    pub fn ensure_worker(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
    ) -> Result<AzureEnsure, AzureError> {
        validate_target(target, &self.profile)?;
        let name = resource_name(workflow_id, job_id);
        let (job, created) = if let Some(job) = self.transport.get(&name)? {
            (job, false)
        } else {
            enforce_cost(target, self.profile.hourly_cost_micros)?;
            let request = create_request(workflow_id, job_id, target, &self.profile)?;
            (self.transport.create(&name, &request)?, true)
        };
        let worker = match worker_from_job(&job, workflow_id, job_id, target, &self.profile) {
            Ok(worker) => worker,
            Err(error) if created => return self.cleanup_creation_failure(&name, workflow_id, job_id, error),
            Err(error) => return Err(error),
        };
        let result = (|| {
            enforce_cost(target, worker.hourly_cost_micros)?;
            let existing = self.execution_for(&job, &worker.resource_id)?;
            let execution = if existing.is_none() && job.ready_to_start() {
                self.transport.start(&name)?
            } else {
                existing
            };
            status_from_resource(&job, &worker, execution.as_ref())
        })();
        let status = match result {
            Ok(status) => status,
            Err(error) if created || matches!(&error, AzureError::HourlyCostRejected { .. }) => {
                return self.cleanup_creation_failure(&name, workflow_id, job_id, error);
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
        let expected = resource_id(
            &self.profile.subscription_id,
            &self.profile.resource_group,
            &worker.name,
        );
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
        if job.ready_to_start() {
            self.single_execution(&job.name, resource_id)
        } else {
            Ok(None)
        }
    }
    fn cleanup_creation_failure<T>(
        &self,
        name: &str,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        error: AzureError,
    ) -> Result<T, AzureError> {
        let owned = self
            .transport
            .get(name)
            .ok()
            .flatten()
            .is_some_and(|job| basic_identity_matches(&job, workflow_id, job_id, &self.profile));
        if !owned || self.transport.delete(name).is_err() {
            return Err(AzureError::CreationCleanupFailed {
                resource_id: resource_id(&self.profile.subscription_id, &self.profile.resource_group, name),
            });
        }
        for delay_millis in [0, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000] {
            std::thread::sleep(std::time::Duration::from_millis(delay_millis));
            if matches!(self.transport.get(name), Ok(None)) {
                return Err(error);
            }
        }
        Err(AzureError::CreationCleanupFailed {
            resource_id: resource_id(&self.profile.subscription_id, &self.profile.resource_group, name),
        })
    }
    #[cfg(test)]
    fn with_transport(profile: AzureProfile, transport: impl Transport + 'static) -> Self {
        Self {
            profile,
            transport: Box::new(transport),
        }
    }
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
    #[error("Azure job creation failed identity verification and exact cleanup also failed")]
    CreationCleanupFailed { resource_id: String },
    #[error("Azure hourly cost {actual} exceeded configured maximum {maximum}")]
    HourlyCostRejected { actual: u64, maximum: u64 },
}
type CreateJobRequest = serde_json::Value;
trait Transport: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<ApiJob>, AzureError>;
    fn create(&self, name: &str, request: &CreateJobRequest) -> Result<ApiJob, AzureError>;
    fn executions(&self, name: &str) -> Result<Vec<ApiExecution>, AzureError>;
    fn start(&self, name: &str) -> Result<Option<ApiExecution>, AzureError>;
    fn delete(&self, name: &str) -> Result<AzureCleanup, AzureError>;
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Registry {
    server: String,
    identity: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiJob {
    id: String,
    name: String,
    location: String,
    tags: BTreeMap<String, String>,
    identity: Option<ApiIdentity>,
    properties: ApiProperties,
}
impl ApiJob {
    fn ready_to_start(&self) -> bool {
        self.properties
            .provisioning_state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case("Succeeded"))
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiIdentity {
    #[serde(rename = "type")]
    kind: String,
    user_assigned_identities: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiProperties {
    environment_id: String,
    provisioning_state: Option<String>,
    configuration: ApiConfiguration,
    template: ApiTemplate,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiConfiguration {
    replica_timeout: u32,
    trigger_type: String,
    registries: Option<Vec<Registry>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ApiTemplate {
    containers: Option<Vec<ApiContainer>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ApiContainer {
    name: String,
    image: String,
    env: Vec<ApiEnv>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiEnv {
    name: String,
    value: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ApiExecution {
    id: String,
    properties: ApiExecutionProperties,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApiExecutionProperties {
    status: Option<String>,
}
fn create_request(
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    profile: &AzureProfile,
) -> Result<CreateJobRequest, AzureError> {
    let deadline = deletion_deadline(target.lease_seconds)?;
    let tags = serde_json::json!({
        OWNER_TAG: "horizon", WORKFLOW_TAG: workflow_id.to_string(), JOB_TAG: job_id.to_string(),
        PROTOCOL_TAG: super::CLOUD_RUN_PROTOCOL_VERSION.to_string(), PROFILE_TAG: profile.name,
        DEADLINE_TAG: deadline, COST_TAG: profile.hourly_cost_micros.to_string(),
        DISK_TAG: target.disk_gib.to_string()
    });
    let env = serde_json::json!([
        {"name": WORKFLOW_ENV, "value": workflow_id.to_string()},
        {"name": JOB_ENV, "value": job_id.to_string()},
        {"name": PROTOCOL_ENV, "value": super::CLOUD_RUN_PROTOCOL_VERSION.to_string()},
        {"name": TERMINATE_ENV, "value": deadline}
    ]);
    let (identity, registries, identity_settings) = profile.registry.as_ref().map_or_else(
        || (serde_json::json!({"type": "None"}), Vec::new(), Vec::new()),
        |registry| {
            (
                serde_json::json!({
                    "type": "UserAssigned", "userAssignedIdentities": {&registry.identity_id: {}}
                }),
                vec![serde_json::json!({"server": registry.server, "identity": registry.identity_id})],
                vec![serde_json::json!({"identity": registry.identity_id, "lifecycle": "None"})],
            )
        },
    );
    let properties = serde_json::json!({
        "environmentId": profile.environment_id,
        "configuration": {
            "triggerType": "Manual", "replicaTimeout": target.lease_seconds, "replicaRetryLimit": 0,
            "manualTriggerConfig": {"parallelism": 1, "replicaCompletionCount": 1},
            "registries": registries, "identitySettings": identity_settings
        },
        "template": {"containers": [{
            "name": "worker", "image": target.image, "env": env,
            "resources": {"cpu": f64::from(profile.cpu_millicores) / 1_000.0,
                          "memory": format!("{:.2}Gi", f64::from(profile.memory_mib) / 1_024.0)}
        }]}
    });
    Ok(serde_json::json!({
        "location": profile.location, "tags": tags, "identity": identity, "properties": properties
    }))
}
fn validate_profile(profile: &AzureProfile) -> Result<(), AzureError> {
    let prefix = format!("/subscriptions/{}/", profile.subscription_id);
    let valid = valid_text(&profile.name, 191)
        && valid_subscription(&profile.subscription_id)
        && valid_resource_group(&profile.resource_group)
        && valid_arm_id(&profile.environment_id)
        && profile
            .environment_id
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        && valid_location(&profile.location)
        && profile.cpu_millicores >= 250
        && profile.cpu_millicores.is_multiple_of(250)
        && profile.memory_mib >= 512
        && profile.memory_mib.is_multiple_of(256)
        && profile.ephemeral_disk_gib > 0
        && profile.hourly_cost_micros > 0
        && profile
            .registry
            .as_ref()
            .is_none_or(|registry| valid_registry_server(&registry.server) && valid_arm_id(&registry.identity_id));
    valid.then_some(()).ok_or(AzureError::InvalidProfile)
}
fn validate_target(target: &WorkerTarget, profile: &AzureProfile) -> Result<(), AzureError> {
    validate_profile(profile)?;
    let registry_matches = profile.registry.as_ref().is_none_or(|registry| {
        target
            .image
            .split_once('/')
            .is_some_and(|(server, _)| server.eq_ignore_ascii_case(&registry.server))
    });
    let valid = target.provider == CloudProvider::Azure
        && target.profile == profile.name
        && (1..=profile.ephemeral_disk_gib).contains(&target.disk_gib)
        && (MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&target.lease_seconds)
        && target.max_hourly_cost_micros != Some(0)
        && valid_immutable_image(&target.image)
        && registry_matches;
    valid.then_some(()).ok_or(AzureError::InvalidTarget)
}
fn enforce_cost(target: &WorkerTarget, actual: u64) -> Result<(), AzureError> {
    let Some(maximum) = target.max_hourly_cost_micros else {
        return Ok(());
    };
    if actual > maximum {
        return Err(AzureError::HourlyCostRejected { actual, maximum });
    }
    Ok(())
}
fn worker_from_job(
    job: &ApiJob,
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    profile: &AzureProfile,
) -> Result<AzureWorker, AzureError> {
    if !basic_identity_matches(job, workflow_id, job_id, profile)
        || !tag_matches(job, PROFILE_TAG, &profile.name)
        || !tag_matches(job, DISK_TAG, &target.disk_gib.to_string())
        || !tag_matches(job, COST_TAG, &profile.hourly_cost_micros.to_string())
        || !job.location.replace(' ', "").eq_ignore_ascii_case(&profile.location)
        || !job_configuration_matches(job, target, profile)
    {
        return Err(AzureError::ResourceIdentityMismatch);
    }
    let delete_after = job
        .tags
        .get(DEADLINE_TAG)
        .filter(|value| valid_deadline(value))
        .cloned()
        .ok_or(AzureError::ResourceIdentityMismatch)?;
    let worker = AzureWorker {
        workflow_id,
        job_id,
        resource_id: resource_id(&profile.subscription_id, &profile.resource_group, &job.name),
        name: job.name.clone(),
        profile: profile.name.clone(),
        environment_id: profile.environment_id.clone(),
        image: target.image.clone(),
        disk_gib: target.disk_gib,
        lease_seconds: target.lease_seconds,
        delete_after,
        hourly_cost_micros: profile.hourly_cost_micros,
    };
    worker.validate()?;
    Ok(worker)
}
fn status_from_resource(
    job: &ApiJob,
    worker: &AzureWorker,
    execution: Option<&ApiExecution>,
) -> Result<AzureWorkerStatus, AzureError> {
    worker.validate()?;
    let containers = job.properties.template.containers.as_deref().unwrap_or_default();
    let owned = job.id.eq_ignore_ascii_case(&worker.resource_id)
        && job.name == worker.name
        && job
            .properties
            .environment_id
            .eq_ignore_ascii_case(&worker.environment_id)
        && job.properties.configuration.replica_timeout == worker.lease_seconds
        && tag_matches(job, OWNER_TAG, "horizon")
        && tag_matches(job, WORKFLOW_TAG, &worker.workflow_id.to_string())
        && tag_matches(job, JOB_TAG, &worker.job_id.to_string())
        && tag_matches(job, PROTOCOL_TAG, &super::CLOUD_RUN_PROTOCOL_VERSION.to_string())
        && tag_matches(job, PROFILE_TAG, &worker.profile)
        && tag_matches(job, DISK_TAG, &worker.disk_gib.to_string())
        && tag_matches(job, COST_TAG, &worker.hourly_cost_micros.to_string())
        && tag_matches(job, DEADLINE_TAG, &worker.delete_after)
        && containers.len() == 1
        && containers[0].name == "worker"
        && containers[0].image == worker.image
        && env_matches(&containers[0], WORKFLOW_ENV, &worker.workflow_id.to_string())
        && env_matches(&containers[0], JOB_ENV, &worker.job_id.to_string())
        && env_matches(
            &containers[0],
            PROTOCOL_ENV,
            &super::CLOUD_RUN_PROTOCOL_VERSION.to_string(),
        )
        && env_matches(&containers[0], TERMINATE_ENV, &worker.delete_after);
    if !owned {
        return Err(AzureError::ResourceIdentityMismatch);
    }
    if let Some(execution) = execution
        && !execution_belongs_to(&execution.id, &worker.resource_id)
    {
        return Err(AzureError::ResourceIdentityMismatch);
    }
    Ok(AzureWorkerStatus {
        worker: worker.clone(),
        lifecycle: lifecycle(job, execution),
        execution_id: execution.map(|value| value.id.clone()),
    })
}
fn lifecycle(job: &ApiJob, execution: Option<&ApiExecution>) -> AzureLifecycle {
    let Some(execution) = execution else {
        return match job.properties.provisioning_state.as_deref() {
            Some(state) if matches_ci(state, &["Failed", "Canceled"]) => AzureLifecycle::Failed,
            Some(state) if state.eq_ignore_ascii_case("Deleting") => AzureLifecycle::Stopped,
            Some(_) => AzureLifecycle::Provisioning,
            None => AzureLifecycle::Unknown,
        };
    };
    match execution.properties.status.as_deref() {
        Some(state) if matches_ci(state, &["Running", "Processing"]) => AzureLifecycle::Running,
        Some(state) if state.eq_ignore_ascii_case("Succeeded") => AzureLifecycle::Succeeded,
        Some(state) if matches_ci(state, &["Failed", "Degraded"]) => AzureLifecycle::Failed,
        Some(state) if state.eq_ignore_ascii_case("Stopped") => AzureLifecycle::Stopped,
        Some(_) | None => AzureLifecycle::Unknown,
    }
}
fn basic_identity_matches(
    job: &ApiJob,
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    profile: &AzureProfile,
) -> bool {
    let name = resource_name(workflow_id, job_id);
    job.name == name
        && job
            .id
            .eq_ignore_ascii_case(&resource_id(&profile.subscription_id, &profile.resource_group, &name))
        && tag_matches(job, OWNER_TAG, "horizon")
        && tag_matches(job, WORKFLOW_TAG, &workflow_id.to_string())
        && tag_matches(job, JOB_TAG, &job_id.to_string())
        && tag_matches(job, PROTOCOL_TAG, &super::CLOUD_RUN_PROTOCOL_VERSION.to_string())
}
fn job_configuration_matches(job: &ApiJob, target: &WorkerTarget, profile: &AzureProfile) -> bool {
    let config = &job.properties.configuration;
    let containers = job.properties.template.containers.as_deref().unwrap_or_default();
    let registry_matches = profile.registry.as_ref().map_or_else(
        || {
            config.registries.as_deref().unwrap_or_default().is_empty()
                && job.identity.as_ref().is_none_or(|identity| {
                    identity.user_assigned_identities.is_empty()
                        && (identity.kind.is_empty() || identity.kind.eq_ignore_ascii_case("None"))
                })
        },
        |registry| {
            config.registries.as_deref()
                == Some(&[Registry {
                    server: registry.server.clone(),
                    identity: registry.identity_id.clone(),
                }])
                && job.identity.as_ref().is_some_and(|identity| {
                    identity.kind.eq_ignore_ascii_case("UserAssigned")
                        && identity.user_assigned_identities.len() == 1
                        && identity
                            .user_assigned_identities
                            .keys()
                            .any(|id| id.eq_ignore_ascii_case(&registry.identity_id))
                })
        },
    );
    config.trigger_type.eq_ignore_ascii_case("Manual")
        && config.replica_timeout == target.lease_seconds
        && containers.len() == 1
        && containers[0].name == "worker"
        && containers[0].image == target.image
        && registry_matches
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
fn tag_matches(job: &ApiJob, key: &str, expected: &str) -> bool {
    job.tags.get(key).is_some_and(|value| value == expected)
}
fn env_matches(container: &ApiContainer, name: &str, expected: &str) -> bool {
    container
        .env
        .iter()
        .find(|entry| entry.name == name)
        .is_some_and(|entry| entry.value.as_deref() == Some(expected))
}
fn execution_belongs_to(execution_id: &str, job_resource_id: &str) -> bool {
    execution_id.len() > job_resource_id.len() + "/executions/".len()
        && execution_id
            .get(..job_resource_id.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(job_resource_id))
        && execution_id[job_resource_id.len()..].starts_with("/executions/")
}
fn matches_ci(value: &str, options: &[&str]) -> bool {
    options.iter().any(|option| value.eq_ignore_ascii_case(option))
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
fn valid_subscription(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
fn valid_resource_group(value: &str) -> bool {
    valid_text(value, 90)
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-()".contains(&byte))
}
fn valid_location(value: &str) -> bool {
    valid_text(value, 90) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
fn valid_registry_server(value: &str) -> bool {
    valid_text(value, 253)
        && value.contains('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
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
