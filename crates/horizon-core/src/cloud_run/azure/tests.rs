use super::*;
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
struct FakeTransport(Arc<Mutex<FakeState>>);
#[derive(Default)]
struct FakeState {
    job: Option<ApiJob>,
    create_response: Option<ApiJob>,
    requests: Vec<CreateJobRequest>,
    executions: Vec<ApiExecution>,
    start_response: Option<ApiExecution>,
    starts: usize,
}
impl Transport for FakeTransport {
    fn get(&self, _name: &str) -> Result<Option<ApiJob>, AzureError> {
        Ok(self.0.lock().expect("state").job.clone())
    }
    fn create(&self, _name: &str, request: &CreateJobRequest) -> Result<ApiJob, AzureError> {
        let mut state = self.0.lock().expect("state");
        state.requests.push(request.clone());
        let job = state.create_response.clone().expect("create response");
        state.job = Some(job.clone());
        Ok(job)
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
        ephemeral_disk_gib: 20,
        hourly_cost_micros: 42_000,
        registry: Some(AzureRegistry {
            server: "registry.example".to_string(),
            identity_id: format!(
                "/subscriptions/{subscription}/resourceGroups/horizon-workers/providers/Microsoft.ManagedIdentity/userAssignedIdentities/pull"
            ),
        }),
    }
}
fn target() -> WorkerTarget {
    WorkerTarget {
        provider: CloudProvider::Azure,
        profile: "general-build".to_string(),
        image: "registry.example/build/worker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        disk_gib: 16,
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
fn execution(job: &ApiJob, status: &str) -> ApiExecution {
    ApiExecution {
        id: format!("{}/executions/{}-abcde", job.id, job.name),
        properties: ApiExecutionProperties {
            status: Some(status.to_string()),
        },
    }
}
#[test]
fn creation_is_bounded_and_retries_reuse_one_execution() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (target, profile) = (target(), profile());
    let job = api_job(workflow_id, job_id, &target, &profile);
    let transport = FakeTransport::default();
    {
        let mut state = transport.0.lock().expect("state");
        state.create_response = Some(job.clone());
        state.start_response = Some(execution(&job, "Processing"));
    }
    let client = AzureClient::with_transport(profile, transport.clone());
    let AzureEnsure::Created(status) = client
        .ensure_worker(workflow_id, job_id, &target)
        .expect("created worker")
    else {
        panic!("new worker must be created");
    };
    assert_eq!(status.lifecycle, AzureLifecycle::Running);
    assert!(status.execution_id.is_some());
    assert!(matches!(
        client.ensure_worker(workflow_id, job_id, &target),
        Ok(AzureEnsure::Reused(_))
    ));
    let state = transport.0.lock().expect("state");
    let request = state.requests.first().expect("request");
    assert_eq!(request["properties"]["configuration"]["replicaTimeout"], 3_600);
    let resources = &request["properties"]["template"]["containers"][0]["resources"];
    assert_eq!(resources["memory"], "1.00Gi");
    assert_eq!(request["identity"]["type"], "UserAssigned");
    assert_eq!(
        request["properties"]["configuration"]["identitySettings"][0]["lifecycle"],
        "None"
    );
    assert_eq!(state.starts, 1);
}
#[test]
fn validation_and_cost_fail_before_creation() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (mut target, profile) = (target(), profile());
    target.max_hourly_cost_micros = Some(41_999);
    let transport = FakeTransport::default();
    let error = AzureClient::with_transport(profile.clone(), transport.clone())
        .ensure_worker(workflow_id, job_id, &target)
        .expect_err("cost rejected");
    assert!(matches!(error, AzureError::HourlyCostRejected { .. }));
    assert!(transport.0.lock().expect("state").requests.is_empty());
    target.max_hourly_cost_micros = None;
    target.disk_gib = 21;
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
    target.disk_gib = 16;
    target.image = "registry.example/build/worker:latest".to_string();
    assert_eq!(validate_target(&target, &profile), Err(AzureError::InvalidTarget));
}
#[test]
fn exact_cleanup_rejects_identity_drift_and_is_idempotent() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let (target, profile) = (target(), profile());
    let good = api_job(workflow_id, job_id, &target, &profile);
    let worker = worker_from_job(&good, workflow_id, job_id, &target, &profile).expect("worker");
    let mut wrong = good.clone();
    wrong.tags.insert(JOB_TAG.to_string(), CloudJobId::new().to_string());
    let transport = FakeTransport::default();
    transport.0.lock().expect("state").job = Some(wrong);
    let client = AzureClient::with_transport(profile.clone(), transport.clone());
    assert_eq!(client.delete_worker(&worker), Err(AzureError::ResourceIdentityMismatch));
    transport.0.lock().expect("state").job = Some(good);
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::Deleted));
    assert_eq!(client.delete_worker(&worker), Ok(AzureCleanup::AlreadyAbsent));
}
