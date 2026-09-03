use super::*;
use AzureCleanup::{AlreadyAbsent, Deleted};
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
struct FakeTransport(Arc<Mutex<FakeState>>);
#[derive(Default)]
struct FakeState {
    job: Option<ApiJob>,
    create_response: Option<ApiJob>,
    job_after_create: Option<ApiJob>,
    requests: Vec<CreateJobRequest>,
    executions: Vec<ApiExecution>,
    create_is_new: bool,
    starts: usize,
}
impl Transport for FakeTransport {
    fn get(&self, _name: &str) -> Result<Option<ApiJob>, AzureError> {
        let mut state = self.0.lock().expect("state");
        if state.job.is_none()
            && !state.requests.is_empty()
            && let Some(job) = state.job_after_create.take()
        {
            state.job = Some(job);
            return Ok(None);
        }
        Ok(state.job.clone())
    }
    fn create(&self, _name: &str, request: &CreateJobRequest) -> Result<CreateResult, AzureError> {
        let mut state = self.0.lock().expect("state");
        state.requests.push(request.clone());
        let job = state.create_response.clone().expect("create response");
        state.job = state.job_after_create.is_none().then(|| job.clone());
        Ok(CreateResult {
            job,
            created: state.create_is_new,
        })
    }
    fn executions(&self, _name: &str) -> Result<Vec<ApiExecution>, AzureError> {
        Ok(self.0.lock().expect("state").executions.clone())
    }
    fn start(&self, _name: &str) -> Result<Option<ApiExecution>, AzureError> {
        let mut state = self.0.lock().expect("state");
        state.starts += 1;
        Ok(None)
    }
    fn delete(&self, _name: &str) -> Result<AzureCleanup, AzureError> {
        let mut state = self.0.lock().expect("state");
        Ok(state.job.take().map_or(AlreadyAbsent, |_| Deleted))
    }
}
fn profile() -> AzureProfile {
    let subscription = "34adfa4f-cedf-4dc0-ba29-b6d1a69ab345";
    AzureProfile {
        name: "general-build".to_string(),
        subscription_id: subscription.to_string(),
        resource_group: "horizon-workers".to_string(),
        environment_id: format!(
            "/subscriptions/{subscription}/resourceGroups/horizon-workers/providers/Microsoft.App/managedEnvironments/shared"
        ),
        location: "swedencentral".to_string(),
        cpu_millicores: 500,
        memory_mib: 1_024,
        hourly_cost_micros: 42_000,
    }
}
fn target() -> WorkerTarget {
    WorkerTarget {
        provider: CloudProvider::Azure,
        profile: "general-build".to_string(),
        image: format!("registry.example/build/worker@sha256:{}", "a".repeat(64)),
        disk_gib: 2,
        lease_seconds: 3_600,
        max_hourly_cost_micros: Some(100_000),
    }
}
fn api_job(workflow: CloudWorkflowId, job: CloudJobId, target: &WorkerTarget, profile: &AzureProfile) -> ApiJob {
    let mut value = create_request(workflow, job, target, profile).expect("request");
    let name = resource_name(workflow, job);
    value["id"] = resource_id(&profile.subscription_id, &profile.resource_group, &name).into();
    value["name"] = name.into();
    value["properties"]["provisioningState"] = "Succeeded".into();
    serde_json::from_value(value).expect("API job")
}
fn assert_mismatch<T>(result: Result<T, AzureError>) {
    assert_eq!(result.err(), Some(AzureError::ResourceIdentityMismatch));
}
#[test]
fn creation_is_bounded_and_retries_reuse_one_execution() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (target, profile) = (target(), profile());
    let job = api_job(workflow_id, job_id, &target, &profile);
    let mut provisioning = job.clone();
    provisioning.properties.provisioning_state = Some("Provisioning".to_string());
    let transport = FakeTransport::default();
    {
        let mut state = transport.0.lock().expect("state");
        state.create_response = Some(provisioning);
        state.job_after_create = Some(job.clone());
        state.create_is_new = true;
    }
    let client = AzureClient::with_transport(profile, transport.clone());
    let created = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(created, Ok(AzureEnsure::Created(_))));
    let reused = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(reused, Ok(AzureEnsure::Reused(_))));
    assert_eq!(transport.0.lock().expect("state").requests.len(), 1);
    transport.0.lock().expect("state").job = None;
    transport.0.lock().expect("state").create_is_new = false;
    let raced = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(raced, Ok(AzureEnsure::Reused(_))));
    assert_eq!(transport.0.lock().expect("state").starts, 1);
}
#[test]
fn validation_and_cost_fail_before_creation() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (mut target, mut profile) = (target(), profile());
    target.max_hourly_cost_micros = Some(41_999);
    let transport = FakeTransport::default();
    let client = AzureClient::with_transport(profile.clone(), transport.clone());
    let error = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(error, Err(AzureError::HourlyCostRejected { .. })));
    assert!(transport.0.lock().expect("state").requests.is_empty());
    target.max_hourly_cost_micros = None;
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
    target.max_hourly_cost_micros = Some(100_000);
    target.disk_gib = 3;
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
    target.disk_gib = 2;
    for deadline in ["2020-01-01T00:00:00Z", "2999-01-01T00:00:00Z"] {
        let mut job = api_job(workflow_id, job_id, &target, &profile);
        job.tags.insert(DEADLINE_TAG.to_string(), deadline.to_string());
        job.properties.template.containers.as_mut().expect("containers")[0].env[3].value = Some(deadline.to_string());
        transport.0.lock().expect("state").job = Some(job);
        assert_mismatch(client.ensure_worker(workflow_id, job_id, &target));
        assert!(transport.0.lock().expect("state").job.is_none());
    }
    target.image = "registry.example/build/worker:latest".to_string();
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
    (profile.cpu_millicores, profile.memory_mib) = (4_250, 8_704);
    assert_eq!(validate_profile(&profile), Err(AzureError::InvalidProfile));
}
#[test]
fn exact_cleanup_rejects_identity_drift_and_is_idempotent() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (target, profile) = (target(), profile());
    let good = api_job(workflow_id, job_id, &target, &profile);
    let worker = worker_from_job(&good, workflow_id, job_id, &target, &profile).expect("worker");
    let mut wrong_profile = good.clone();
    wrong_profile.properties.workload_profile_name = "Dedicated".to_string();
    assert_mismatch(worker_from_job(&wrong_profile, workflow_id, job_id, &target, &profile));
    wrong_profile.properties.workload_profile_name = CONSUMPTION_PROFILE.to_string();
    wrong_profile.location = "eastus".to_string();
    assert_mismatch(status_from_resource(&wrong_profile, &worker, None));
    let mut wrong = good.clone();
    wrong.properties.template.containers.as_mut().expect("containers")[0].env[1].secret_ref =
        Some("job-override".to_string());
    assert_mismatch(status_from_resource(&wrong, &worker, None));
    let containers = wrong.properties.template.containers.as_mut().expect("containers");
    containers[0].env[1].secret_ref = None;
    let duplicate = containers[0].env[1].clone();
    containers[0].env.push(duplicate);
    let transport = FakeTransport::default();
    transport.0.lock().expect("state").create_response = Some(wrong);
    transport.0.lock().expect("state").create_is_new = true;
    let client = AzureClient::with_transport(profile, transport.clone());
    let failed = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(failed, Err(AzureError::CleanupFailed { .. })));
    assert_eq!(transport.0.lock().expect("state").starts, 0);
    assert!(transport.0.lock().expect("state").job.is_some());
    transport.0.lock().expect("state").job = Some(good);
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::Deleted));
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::AlreadyAbsent));
    let absent = client.cleanup_creation_failure::<()>(&worker.name, Some(&worker), AzureError::InvalidTarget);
    assert_eq!(absent, Err(AzureError::InvalidTarget));
}
