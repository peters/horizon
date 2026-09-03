use super::super::{CloudJobId, CloudWorkflowId, WorkerTarget};
use super::{
    AzureError, AzureLifecycle, AzureProfile, AzureWorker, AzureWorkerStatus, CONSUMPTION_PROFILE, COST_TAG,
    DEADLINE_TAG, DISK_TAG, JOB_ENV, JOB_TAG, OWNER_TAG, PROFILE_TAG, PROTOCOL_ENV, PROTOCOL_TAG, TERMINATE_ENV,
    WORKFLOW_ENV, WORKFLOW_TAG, resource_id, resource_name, valid_deadline,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub(super) type CreateJobRequest = serde_json::Value;
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Registry {
    pub(super) server: String,
    pub(super) identity: String,
    pub(super) username: Option<String>,
    pub(super) password_secret_ref: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct IdentitySetting {
    pub(super) identity: String,
    pub(super) lifecycle: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiJob {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) location: String,
    pub(super) tags: BTreeMap<String, String>,
    pub(super) identity: Option<ApiIdentity>,
    pub(super) properties: ApiProperties,
}
impl ApiJob {
    pub(super) fn ready_to_start(&self) -> bool {
        self.properties
            .provisioning_state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case("Succeeded"))
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ApiIdentity {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) user_assigned_identities: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ApiProperties {
    pub(super) environment_id: String,
    pub(super) workload_profile_name: String,
    pub(super) provisioning_state: Option<String>,
    pub(super) configuration: ApiConfiguration,
    pub(super) template: ApiTemplate,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ApiConfiguration {
    pub(super) replica_timeout: u32,
    pub(super) replica_retry_limit: u32,
    pub(super) trigger_type: String,
    pub(super) manual_trigger_config: ApiManualTriggerConfig,
    pub(super) registries: Option<Vec<Registry>>,
    pub(super) identity_settings: Option<Vec<IdentitySetting>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ApiManualTriggerConfig {
    pub(super) parallelism: u32,
    pub(super) replica_completion_count: u32,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiTemplate {
    pub(super) containers: Option<Vec<ApiContainer>>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiContainer {
    pub(super) name: String,
    pub(super) image: String,
    pub(super) env: Vec<ApiEnv>,
    pub(super) resources: ApiResources,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiResources {
    pub(super) cpu: f64,
    pub(super) memory: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ApiEnv {
    pub(super) name: String,
    pub(super) value: Option<String>,
    pub(super) secret_ref: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiExecution {
    pub(super) id: String,
    pub(super) properties: ApiExecutionProperties,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ApiExecutionProperties {
    pub(super) status: Option<String>,
}
pub(super) fn create_request(
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    profile: &AzureProfile,
) -> Result<CreateJobRequest, AzureError> {
    let deadline = super::deletion_deadline(target.lease_seconds)?;
    let tags = serde_json::json!({
        OWNER_TAG: "horizon", WORKFLOW_TAG: workflow_id.to_string(), JOB_TAG: job_id.to_string(),
        PROTOCOL_TAG: super::super::CLOUD_RUN_PROTOCOL_VERSION.to_string(), PROFILE_TAG: profile.name,
        DEADLINE_TAG: deadline, COST_TAG: profile.hourly_cost_micros.to_string(), DISK_TAG: target.disk_gib.to_string()
    });
    let env = serde_json::json!([
        {"name": WORKFLOW_ENV, "value": workflow_id.to_string()},
        {"name": JOB_ENV, "value": job_id.to_string()},
        {"name": PROTOCOL_ENV, "value": super::super::CLOUD_RUN_PROTOCOL_VERSION.to_string()},
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
        "environmentId": profile.environment_id, "workloadProfileName": CONSUMPTION_PROFILE,
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
pub(super) fn worker_from_job(
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
        || !job_configuration_matches(job, &target.image, target.lease_seconds, profile)
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
        target: target.clone(),
        profile: profile.clone(),
        delete_after,
    };
    worker.validate()?;
    Ok(worker)
}
pub(super) fn status_from_resource(
    job: &ApiJob,
    worker: &AzureWorker,
    execution: Option<&ApiExecution>,
) -> Result<AzureWorkerStatus, AzureError> {
    worker.validate()?;
    let properties = &job.properties;
    let containers = properties.template.containers.as_deref().unwrap_or_default();
    let (target, profile) = (&worker.target, &worker.profile);
    let protocol = super::super::CLOUD_RUN_PROTOCOL_VERSION.to_string();
    let owned = basic_identity_matches(job, worker.workflow_id, worker.job_id, profile)
        && job.id.eq_ignore_ascii_case(&worker.resource_id)
        && job.location.replace(' ', "").eq_ignore_ascii_case(&profile.location)
        && properties.environment_id.eq_ignore_ascii_case(&profile.environment_id)
        && job_configuration_matches(job, &target.image, target.lease_seconds, profile)
        && tag_matches(job, PROFILE_TAG, &profile.name)
        && tag_matches(job, DISK_TAG, &target.disk_gib.to_string())
        && tag_matches(job, COST_TAG, &profile.hourly_cost_micros.to_string())
        && tag_matches(job, DEADLINE_TAG, &worker.delete_after)
        && containers.len() == 1
        && env_matches(&containers[0], WORKFLOW_ENV, &worker.workflow_id.to_string())
        && env_matches(&containers[0], JOB_ENV, &worker.job_id.to_string())
        && env_matches(&containers[0], PROTOCOL_ENV, &protocol)
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
    let id = resource_id(&profile.subscription_id, &profile.resource_group, &name);
    job.name == name
        && job.id.eq_ignore_ascii_case(&id)
        && tag_matches(job, OWNER_TAG, "horizon")
        && tag_matches(job, WORKFLOW_TAG, &workflow_id.to_string())
        && tag_matches(job, JOB_TAG, &job_id.to_string())
        && tag_matches(job, PROTOCOL_TAG, &super::super::CLOUD_RUN_PROTOCOL_VERSION.to_string())
}
fn job_configuration_matches(job: &ApiJob, image: &str, lease_seconds: u32, profile: &AzureProfile) -> bool {
    let config = &job.properties.configuration;
    let workload_profile = &job.properties.workload_profile_name;
    let containers = job.properties.template.containers.as_deref().unwrap_or_default();
    let registry_matches = profile.registry.as_ref().map_or_else(
        || {
            config.registries.as_deref().unwrap_or_default().is_empty()
                && config.identity_settings.as_deref().unwrap_or_default().is_empty()
                && job.identity.as_ref().is_none_or(|identity| {
                    identity.user_assigned_identities.is_empty()
                        && (identity.kind.is_empty() || identity.kind.eq_ignore_ascii_case("None"))
                })
        },
        |registry| {
            matches!(
                config.registries.as_deref(),
                Some([actual])
                    if actual.server.eq_ignore_ascii_case(&registry.server)
                        && actual.identity.eq_ignore_ascii_case(&registry.identity_id)
                        && actual.username.is_none()
                        && actual.password_secret_ref.is_none()
            ) && job.identity.as_ref().is_some_and(|identity| {
                identity.kind.eq_ignore_ascii_case("UserAssigned")
                    && identity.user_assigned_identities.len() == 1
                    && identity
                        .user_assigned_identities
                        .keys()
                        .any(|id| id.eq_ignore_ascii_case(&registry.identity_id))
            }) && config.identity_settings.as_deref().is_some_and(|settings| {
                settings.len() == 1
                    && settings[0].identity.eq_ignore_ascii_case(&registry.identity_id)
                    && settings[0].lifecycle.eq_ignore_ascii_case("None")
            })
        },
    );
    config.trigger_type.eq_ignore_ascii_case("Manual")
        && workload_profile.eq_ignore_ascii_case(CONSUMPTION_PROFILE)
        && config.replica_timeout == lease_seconds
        && config.replica_retry_limit == 0
        && config.manual_trigger_config.parallelism == 1
        && config.manual_trigger_config.replica_completion_count == 1
        && containers.len() == 1
        && containers[0].name == "worker"
        && containers[0].image == image
        && (containers[0].resources.cpu - f64::from(profile.cpu_millicores) / 1_000.0).abs() < f64::EPSILON
        && memory_matches(&containers[0].resources.memory, profile.memory_mib)
        && registry_matches
}
fn memory_matches(value: &str, expected_mib: u32) -> bool {
    value
        .strip_suffix("Gi")
        .and_then(|amount| amount.parse::<f64>().ok())
        .is_some_and(|amount| (amount - f64::from(expected_mib) / 1_024.0).abs() < f64::EPSILON)
}
fn tag_matches(job: &ApiJob, key: &str, expected: &str) -> bool {
    job.tags.get(key).is_some_and(|value| value == expected)
}
fn env_matches(container: &ApiContainer, name: &str, expected: &str) -> bool {
    let mut entries = container.env.iter().filter(|entry| entry.name == name);
    entries
        .next()
        .is_some_and(|entry| entry.value.as_deref() == Some(expected) && entry.secret_ref.is_none())
        && entries.next().is_none()
}
pub(super) fn execution_belongs_to(execution_id: &str, job_resource_id: &str) -> bool {
    execution_id.len() > job_resource_id.len() + "/executions/".len()
        && execution_id
            .get(..job_resource_id.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(job_resource_id))
        && execution_id[job_resource_id.len()..].starts_with("/executions/")
}
fn matches_ci(value: &str, options: &[&str]) -> bool {
    options.iter().any(|option| value.eq_ignore_ascii_case(option))
}
