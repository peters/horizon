use super::*;
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
    start_response: Option<ApiExecution>,
    create_is_new: bool,
    starts: usize,
}
impl Transport for FakeTransport {
    fn get(&self, _name: &str) -> Result<Option<ApiJob>, AzureError> {
        Ok(self.0.lock().expect("state").job.clone())
    }
    fn create(&self, _name: &str, request: &CreateJobRequest) -> Result<CreateResult, AzureError> {
        let mut state = self.0.lock().expect("state");
        state.requests.push(request.clone());
        let job = state.create_response.clone().expect("create response");
        state.job = Some(state.job_after_create.clone().unwrap_or_else(|| job.clone()));
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
        let execution = state.start_response.clone();
        state.executions.extend(execution.clone());
        Ok(execution)
    }
    fn delete(&self, _name: &str) -> Result<AzureCleanup, AzureError> {
        let mut state = self.0.lock().expect("state");
        if state.job.take().is_none() {
            return Ok(AzureCleanup::AlreadyAbsent);
        }
        Ok(AzureCleanup::Deleted)
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
        image: "registry.example/build/worker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
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
    let AzureEnsure::Created(status) = client
        .ensure_worker(workflow_id, job_id, &target)
        .expect("created worker")
    else {
        panic!("new worker must be created");
    };
    assert_eq!(status.lifecycle, AzureLifecycle::Provisioning);
    let reused = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(reused, Ok(AzureEnsure::Reused(_))));
    let state = transport.0.lock().expect("state");
    assert_eq!(state.requests.len(), 1);
    assert_eq!(state.starts, 1);
    drop(state);
    transport.0.lock().expect("state").job = None;
    transport.0.lock().expect("state").create_is_new = false;
    let raced = client.ensure_worker(workflow_id, job_id, &target);
    assert!(matches!(raced, Ok(AzureEnsure::Reused(_))));
    assert_eq!(transport.0.lock().expect("state").starts, 1);
}
#[test]
fn validation_and_cost_fail_before_creation() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (mut target, profile) = (target(), profile());
    target.max_hourly_cost_micros = Some(41_999);
    let transport = FakeTransport::default();
    let client = AzureClient::with_transport(profile.clone(), transport.clone());
    let error = client
        .ensure_worker(workflow_id, job_id, &target)
        .expect_err("cost rejected");
    assert!(matches!(error, AzureError::HourlyCostRejected { .. }));
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
        transport.0.lock().expect("state").job = Some(job);
        let result = client.ensure_worker(workflow_id, job_id, &target);
        assert_eq!(result, Err(AzureError::ResourceIdentityMismatch));
        assert!(transport.0.lock().expect("state").job.is_none());
    }
    target.image = "registry.example/build/worker:latest".to_string();
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
}
#[test]
fn exact_cleanup_rejects_identity_drift_and_is_idempotent() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (target, profile) = (target(), profile());
    let good = api_job(workflow_id, job_id, &target, &profile);
    let worker = worker_from_job(&good, workflow_id, job_id, &target, &profile).expect("worker");
    let mut oversized = good.clone();
    oversized.properties.template.containers.as_mut().expect("containers")[0]
        .resources
        .cpu = 1.0;
    let mismatch = worker_from_job(&oversized, workflow_id, job_id, &target, &profile);
    assert_eq!(mismatch, Err(AzureError::ResourceIdentityMismatch));
    let mut wrong = good.clone();
    let containers = wrong.properties.template.containers.as_mut().expect("containers");
    let duplicate = containers[0].env[1].clone();
    containers[0].env.push(duplicate);
    let transport = FakeTransport::default();
    transport.0.lock().expect("state").job = Some(wrong);
    let mut edited_profile = profile.clone();
    edited_profile.cpu_millicores = 750;
    let client = AzureClient::with_transport(edited_profile, transport.clone());
    assert_eq!(client.delete_worker(&worker), Err(AzureError::ResourceIdentityMismatch));
    transport.0.lock().expect("state").job = Some(good);
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::Deleted));
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::AlreadyAbsent));
}
