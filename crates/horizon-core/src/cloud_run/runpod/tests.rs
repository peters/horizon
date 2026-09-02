use super::*;
use crate::cloud_run::CloudProvider;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct FakeTransport {
    state: Arc<Mutex<FakeState>>,
}
#[derive(Default)]
struct FakeState {
    pods: Vec<ApiPod>,
    create_response: Option<ApiPod>,
    create_requests: Vec<CreatePodRequest>,
    deleted: Vec<String>,
    fail_delete: bool,
}

impl FakeTransport {
    fn with_create_response(response: ApiPod) -> Self {
        let transport = Self::default();
        transport.state.lock().expect("state").create_response = Some(response);
        transport
    }
    fn with_pods(pods: Vec<ApiPod>) -> Self {
        let transport = Self::default();
        transport.state.lock().expect("state").pods = pods;
        transport
    }
}
impl Transport for FakeTransport {
    fn list_by_name(&self, _name: &str) -> Result<Vec<ApiPod>, RunPodError> {
        Ok(self.state.lock().expect("state").pods.clone())
    }
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        let mut state = self.state.lock().expect("state");
        state.create_requests.push(request.clone());
        let mut response = state.create_response.clone().expect("create response");
        response
            .env
            .insert(TERMINATE_ENV.to_string(), request.terminate_after.clone());
        state.pods.push(response.clone());
        Ok(response)
    }
    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError> {
        Ok(self
            .state
            .lock()
            .expect("state")
            .pods
            .iter()
            .find(|pod| pod.id == pod_id)
            .cloned())
    }
    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError> {
        let mut state = self.state.lock().expect("state");
        if state.fail_delete {
            return Err(RunPodError::RequestFailed {
                operation: "pod deletion",
            });
        }
        state.deleted.push(pod_id.to_string());
        state.pods.retain(|pod| pod.id != pod_id);
        Ok(RunPodCleanup::Deleted)
    }
}
fn target() -> WorkerTarget {
    WorkerTarget {
        provider: CloudProvider::RunPod,
        profile: "nativesdk-gpu".to_string(),
        image:
            "registry.example/team/nativesdk@sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                .to_string(),
        disk_gib: 80,
        lease_seconds: 3_600,
        max_hourly_cost_micros: Some(750_000),
    }
}
fn profile() -> RunPodProfile {
    RunPodProfile {
        name: "nativesdk-gpu".to_string(),
        gpu_type_ids: vec!["NVIDIA RTX A4000".to_string(), "NVIDIA RTX A4500".to_string()],
        gpu_count: 1,
        allowed_cuda_versions: vec!["12.8".to_string()],
        data_center_ids: vec!["EUR-NO-1".to_string()],
        ports: vec!["22/tcp".to_string()],
        volume_gib: 20,
        min_download_mbps: Some(250),
        min_upload_mbps: Some(100),
        min_disk_bandwidth_mbps: Some(200),
        container_registry_auth_id: Some("registry_auth-1".to_string()),
    }
}
fn api_pod(
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    hourly_cost_micros: Option<u64>,
) -> ApiPod {
    let terminate_after = termination_deadline(target.lease_seconds).expect("termination deadline");
    ApiPod {
        id: "pod_123456".to_string(),
        name: resource_name(workflow_id, job_id),
        image: target.image.clone(),
        status: Some("PROVISIONING".to_string()),
        ssh: Some(ApiSsh::default()),
        env: BTreeMap::from([
            (WORKFLOW_ENV.to_string(), workflow_id.to_string()),
            (JOB_ENV.to_string(), job_id.to_string()),
            (
                PROTOCOL_ENV.to_string(),
                super::super::CLOUD_RUN_PROTOCOL_VERSION.to_string(),
            ),
            (TERMINATE_ENV.to_string(), terminate_after),
        ]),
        cost: hourly_cost_micros,
    }
}
#[test]
fn api_key_validation_never_exposes_secret() {
    let key = RunPodApiKey::new("rpa_example-secret").expect("valid API key");
    assert_eq!(format!("{key:?}"), "RunPodApiKey(<redacted>)");
    for invalid in ["", "with space", "line\nbreak", "tab\tvalue", "é"] {
        let error = RunPodApiKey::new(invalid).expect_err("invalid key");
        assert_eq!(error, RunPodError::InvalidApiKey);
        assert!(invalid.is_empty() || !error.to_string().contains(invalid));
    }
}

#[test]
fn provider_messages_classify_capacity_shortage() {
    let exhausted = serde_json::json!({"errors": [
        {"message": "There are no instances currently available"},
        {"message": "This machine does not have the resources; try a different machine"}
    ]});
    assert!(http::capacity_unavailable(&exhausted));
    assert!(!http::capacity_unavailable(&serde_json::json!({
        "errors": [{"message": "invalid image reference"}]
    })));
}

#[test]
fn v2_cold_start_accepts_null_runtime_and_ssh_endpoints() {
    let pod: ApiPod = serde_json::from_value(serde_json::json!({
        "id": "pod_123456", "name": "cold-start", "image": target().image,
        "status": "PROVISIONING", "env": {}, "cost": 0.24,
        "ssh": {"proxy": null, "direct": null}, "runtime": null
    }))
    .expect("valid v2 cold-start pod");
    assert!(pod.ssh.is_some_and(|ssh| ssh.direct.is_none()));
}

#[test]
fn creation_uses_secure_direct_pull_and_bandwidth_constraints() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let mut mutable_target = target.clone();
    mutable_target.image = "registry.example/team/nativesdk:latest".to_string();
    assert_eq!(
        validate_target(&mutable_target, &profile()),
        Err(RunPodError::InvalidTarget)
    );
    let response = api_pod(workflow_id, job_id, &target, Some(900_001));
    let transport = FakeTransport::with_create_response(response);
    let observer = transport.clone();
    let error = RunPodClient::with_transport(transport)
        .ensure_worker(workflow_id, job_id, &target, &profile())
        .expect_err("cost rejected after creation");
    assert!(matches!(error, RunPodError::HourlyCostRejected { .. }));

    let state = observer.state.lock().expect("state");
    let request = state.create_requests.first().expect("creation request");
    assert_eq!(request.cloud_type, "SECURE");
    assert_eq!(request.image_name, target.image);
    assert_eq!(request.min_download_mbps, Some(250));
    assert_eq!(request.min_upload_mbps, Some(100));
    assert_eq!(request.container_disk_in_gb, 80);
    assert_eq!(request.env.len(), 4);
    assert!(
        request
            .env
            .iter()
            .any(|entry| entry.key == TERMINATE_ENV && entry.value == request.terminate_after)
    );
    let json = serde_json::to_value(request).expect("serialize request");
    assert!(json.to_string().find("rpa_").is_none());
    assert_eq!(state.deleted, ["pod_123456"]);
}

#[test]
fn retries_reuse_one_exact_worker_and_reject_ambiguity() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let pod = api_pod(workflow_id, job_id, &target, Some(420_000));
    let transport = FakeTransport::with_pods(vec![pod.clone()]);
    let observer = transport.clone();
    let result = RunPodClient::with_transport(transport)
        .ensure_worker(workflow_id, job_id, &target, &profile())
        .expect("worker reused");
    assert!(matches!(result, RunPodEnsure::Reused(_)));
    assert!(observer.state.lock().expect("state").create_requests.is_empty());

    let client = RunPodClient::with_transport(FakeTransport::with_pods(vec![pod.clone(), pod]));
    assert!(matches!(
        client.ensure_worker(workflow_id, job_id, &target, &profile()),
        Err(RunPodError::AmbiguousResource { count: 2, .. })
    ));
}

#[test]
fn identity_mismatch_blocks_delete_and_exact_delete_is_idempotent() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let good = api_pod(workflow_id, job_id, &target, Some(420_000));
    let result = RunPodClient::with_transport(FakeTransport::with_pods(vec![good.clone()]))
        .ensure_worker(workflow_id, job_id, &target, &profile())
        .expect("worker reused");
    let RunPodEnsure::Reused(status) = result else {
        panic!("existing worker must be reused");
    };
    let worker = status.worker;
    let mut mutable_worker = worker.clone();
    mutable_worker.image = "registry.example/team/nativesdk:latest".to_string();
    assert_eq!(mutable_worker.validate(), Err(RunPodError::InvalidPersistedWorker));

    let mut wrong = good.clone();
    wrong.env.insert(JOB_ENV.to_string(), CloudJobId::new().to_string());
    let transport = FakeTransport::with_pods(vec![wrong]);
    let observer = transport.clone();
    let client = RunPodClient::with_transport(transport);
    assert_eq!(
        client.delete_worker(&worker),
        Err(RunPodError::ResourceIdentityMismatch)
    );
    assert!(observer.state.lock().expect("state").deleted.is_empty());

    let transport = FakeTransport::with_pods(vec![good]);
    let observer = transport.clone();
    let client = RunPodClient::with_transport(transport);
    assert_eq!(client.delete_worker(&worker), Ok(RunPodCleanup::Deleted));
    assert_eq!(client.inspect_worker(&worker), Ok(None));
    assert_eq!(client.delete_worker(&worker), Ok(RunPodCleanup::AlreadyAbsent));
    assert_eq!(observer.state.lock().expect("state").deleted, [worker.pod_id]);
}
