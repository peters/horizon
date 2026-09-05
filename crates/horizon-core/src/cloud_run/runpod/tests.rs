use super::super::WorkerLifetime;
use super::super::interactive_worker::{
    InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
    InteractiveWorkerLease, InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider,
    InteractiveWorkerRequest,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{Arc, Mutex};

const ED25519_BLOB_PREFIX: &[u8] = b"\0\0\0\x0bssh-ed25519\0\0\0\x20";

#[derive(Clone, Default)]
struct FakeTransport(Arc<Mutex<FakeState>>);
#[derive(Default)]
struct FakeState {
    list_calls: usize,
    pods: Vec<ApiPod>,
    create_response: Option<ApiPod>,
    create_requests: Vec<CreatePodRequest>,
    deleted: Vec<String>,
    inspected: Vec<String>,
    scripted_lists: Vec<Vec<ApiPod>>,
}
impl FakeTransport {
    fn with_create_response(response: ApiPod) -> Self {
        let transport = Self::default();
        transport.0.lock().expect("state").create_response = Some(response);
        transport
    }
    fn with_pods(pods: Vec<ApiPod>) -> Self {
        let transport = Self::default();
        transport.0.lock().expect("state").pods = pods;
        transport
    }
}
impl Transport for FakeTransport {
    fn list_by_name(&self, _name: &str) -> Result<Vec<ApiPod>, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.list_calls += 1;
        if !state.scripted_lists.is_empty() {
            return Ok(state.scripted_lists.remove(0));
        }
        Ok(state.pods.clone())
    }
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.create_requests.push(request.clone());
        let mut response = state.create_response.clone().expect("create response");
        for entry in &request.env {
            response.env.insert(entry.key.clone(), entry.value.clone());
        }
        state.pods.push(response.clone());
        Ok(response)
    }
    fn get(&self, pod_id: &str) -> Result<Option<ApiPod>, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.inspected.push(pod_id.to_string());
        Ok(state.pods.iter().find(|pod| pod.id == pod_id).cloned())
    }
    fn delete(&self, pod_id: &str) -> Result<RunPodCleanup, RunPodError> {
        let mut state = self.0.lock().expect("state");
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
        lifetime: WorkerLifetime::TimeLimited { seconds: 3_600 },
        max_hourly_cost_micros: Some(750_000),
    }
}
fn profile() -> RunPodProfile {
    RunPodProfile {
        name: "nativesdk-gpu".to_string(),
        gpu_type_ids: vec!["NVIDIA RTX A4000".to_string(), "NVIDIA RTX A4500".to_string()],
        gpu_count: 1,
        allowed_cuda_versions: vec!["12.8".to_string()],
        data_center_id: Some("EUR-NO-1".to_string()),
        ports: vec!["22/tcp".to_string()],
        volume_gib: 20,
        min_download_mbps: Some(250),
        min_upload_mbps: Some(100),
        min_disk_bandwidth_mbps: Some(200),
        container_registry_auth_id: Some("registry_auth-1".to_string()),
    }
}

fn ed25519_key(byte: u8) -> String {
    let mut blob = ED25519_BLOB_PREFIX.to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn api_pod(
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
    target: &WorkerTarget,
    hourly_cost_micros: Option<u64>,
) -> ApiPod {
    let terminate_after = termination_deadline(target.lifetime.time_limit_seconds().expect("time-limited fixture"))
        .expect("termination deadline");
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

fn interactive_request(workflow_id: CloudWorkflowId, job_id: CloudJobId) -> InteractiveWorkerRequest {
    InteractiveWorkerRequest {
        workflow_id,
        job_id,
        target: target(),
        ssh_public_key: ed25519_key(41),
    }
}

fn running_pod(workflow_id: CloudWorkflowId, job_id: CloudJobId, target: &WorkerTarget) -> ApiPod {
    let mut pod = api_pod(workflow_id, job_id, target, Some(420_000));
    pod.status = Some("RUNNING".to_string());
    pod.ssh = Some(ApiSsh {
        direct: Some(ApiSshEndpoint {
            username: "root".to_string(),
            host: "worker.example".to_string(),
            port: 22,
        }),
    });
    pod
}

#[derive(Clone)]
struct FakeHostKeySource {
    host_key: Option<String>,
    calls: Arc<Mutex<Vec<(String, String, u16)>>>,
}

impl FakeHostKeySource {
    fn new(host_key: Option<String>) -> Self {
        Self {
            host_key,
            calls: Arc::default(),
        }
    }
}

impl RunPodHostKeySource for FakeHostKeySource {
    fn host_key(&self, worker: &RunPodWorker, endpoint: &RunPodSshEndpoint) -> Option<String> {
        self.calls
            .lock()
            .expect("host key calls")
            .push((worker.pod_id.clone(), endpoint.host.clone(), endpoint.port));
        self.host_key.clone()
    }
}
#[test]
fn api_key_validation_never_exposes_secret() {
    let key = RunPodApiKey::new("rpa_example-secret").expect("valid API key");
    assert_eq!(format!("{key:?}"), "RunPodApiKey(<redacted>)");
    for invalid in ["", "with space", "line\nbreak", "tab\tvalue", "é"] {
        let error = RunPodApiKey::new(invalid).expect_err("invalid key");
        assert_eq!(error, RunPodError::InvalidApiKey);
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
    assert_eq!(pod.cost, Some(240_000));
    assert_eq!(decimal_micros("0.0000001"), Some(1));
}
#[test]
fn creation_uses_secure_direct_pull_and_bandwidth_constraints() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let mut mutable_target = target.clone();
    mutable_target.image = "registry.example/team/nativesdk@sha256:d".to_string();
    assert_eq!(
        validate_target(&mutable_target, &profile()),
        Err(RunPodError::InvalidTarget)
    );
    let mut unreachable = profile();
    unreachable.ports.clear();
    assert_eq!(validate_target(&target, &unreachable), Err(RunPodError::InvalidTarget));
    let response = api_pod(workflow_id, job_id, &target, Some(0));
    let transport = FakeTransport::with_create_response(response);
    let observer = transport.clone();
    let error = RunPodClient::with_transport(transport)
        .ensure_worker(workflow_id, job_id, &target, &profile())
        .expect_err("cost rejected after creation");
    assert!(matches!(error, RunPodError::HourlyCostRejected { actual: None, .. }));
    let state = observer.0.lock().expect("state");
    let request = state.create_requests.first().expect("creation request");
    assert_eq!(request.cloud_type, "SECURE");
    assert_eq!(request.image_name, target.image);
    assert_eq!(request.min_download_mbps, Some(250));
    assert!(
        request
            .env
            .iter()
            .any(|entry| entry.key == TERMINATE_ENV && entry.value == request.terminate_after)
    );
    let json = serde_json::to_value(request).expect("serialize request");
    assert_eq!(json["dataCenterId"], "EUR-NO-1");
    assert!(json.get("dataCenterIds").is_none());
    assert!(json.to_string().find("rpa_").is_none());
    assert_eq!(state.deleted, ["pod_123456"]);
}
#[test]
fn retries_reuse_one_exact_worker_and_reject_ambiguity() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let pod = api_pod(workflow_id, job_id, &target, Some(420_000));
    let transport = FakeTransport::with_pods(vec![pod.clone()]);
    transport.0.lock().expect("state").scripted_lists = vec![vec![], vec![]];
    let observer = transport.clone();
    let result = RunPodClient::with_transport(transport)
        .ensure_worker(workflow_id, job_id, &target, &profile())
        .expect("worker reused");
    assert!(matches!(result, RunPodEnsure::Reused(_)));
    assert!(observer.0.lock().expect("state").create_requests.is_empty());
    let mut late = pod.clone();
    late.id = "pod_654321".to_string();
    let transport = FakeTransport::with_pods(vec![pod.clone(), late]);
    transport.0.lock().expect("state").scripted_lists = vec![vec![pod.clone()]];
    let client = RunPodClient::with_transport(transport);
    assert!(matches!(
        client.ensure_worker(workflow_id, job_id, &target, &profile()),
        Err(RunPodError::AmbiguousResource { count: 2, .. })
    ));
    let transport = FakeTransport::with_create_response(api_pod(workflow_id, job_id, &target, Some(420_000)));
    let observer = transport.clone();
    let expected_target = target.clone();
    let client = RunPodClient {
        transport: Box::new(transport),
        creation_fence: Box::new(
            move |actual_workflow, actual_job, actual_target: &WorkerTarget, name: &str| {
                assert_eq!(actual_workflow, workflow_id);
                assert_eq!(actual_job, job_id);
                assert_eq!(actual_target, &expected_target);
                assert_eq!(name, resource_name(workflow_id, job_id));
                Ok(false)
            },
        ),
    };
    assert!(matches!(
        client.ensure_worker(workflow_id, job_id, &target, &profile()),
        Err(RunPodError::CreationUnresolved { .. })
    ));
    assert!(observer.0.lock().expect("state").create_requests.is_empty());
    for deadline in ["2000-01-01T00:00:00Z", "2999-01-01T00:00:00Z"] {
        let mut invalid = api_pod(workflow_id, job_id, &target, Some(420_000));
        invalid.env.insert(TERMINATE_ENV.to_string(), deadline.to_string());
        let transport = FakeTransport::with_pods(vec![invalid]);
        let observer = transport.clone();
        assert!(matches!(
            RunPodClient::with_transport(transport).ensure_worker(workflow_id, job_id, &target, &profile()),
            Err(RunPodError::LeaseDeadlineRejected { .. })
        ));
        assert!(observer.0.lock().expect("state").pods.is_empty());
    }
}
#[test]
fn identity_mismatch_blocks_delete_and_exact_delete_is_idempotent() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let target = target();
    let mut good = api_pod(workflow_id, job_id, &target, Some(420_000));
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
    assert!(observer.0.lock().expect("state").deleted.is_empty());
    good.cost = Some(0);
    good.ssh = serde_json::from_str(r#"{"direct":{"username":"root","host":"ssh.example","port":22}}"#).expect("ssh");
    let transport = FakeTransport::with_pods(vec![good]);
    let observer = transport.clone();
    let client = RunPodClient::with_transport(transport);
    assert!(
        matches!(client.inspect_worker(&worker), Ok(Some(status)) if status.worker.hourly_cost_micros == Some(420_000) && status.ssh_username.as_deref() == Some("root"))
    );
    assert_eq!(client.delete_worker(&worker), Ok(RunPodCleanup::Deleted));
    assert_eq!(client.delete_worker(&worker), Ok(RunPodCleanup::AlreadyAbsent));
    assert_eq!(observer.0.lock().expect("state").deleted, [worker.pod_id]);
}

#[test]
fn interactive_adapter_preserves_identity_and_reaches_attested_ready_state() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let request = interactive_request(workflow_id, job_id);
    let transport = FakeTransport::with_create_response(running_pod(workflow_id, job_id, &request.target));
    let observer = transport.clone();
    let host_keys = FakeHostKeySource::new(Some(ed25519_key(73)));
    let host_key_calls = host_keys.calls.clone();
    let provider = RunPodInteractiveWorkerProvider::new(RunPodClient::with_transport(transport), profile(), host_keys);

    assert_eq!(provider.provider(), CloudProvider::RunPod);
    let InteractiveWorkerEnsure::Created(status) = provider.ensure_worker(&request).expect("create interactive worker")
    else {
        panic!("new worker must be created");
    };
    assert_eq!(status.worker.identity.resource_id, "pod_123456");
    assert_eq!(status.worker.ssh_public_key, request.ssh_public_key);
    assert!(status.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_eq!(
        status.ssh.as_ref().map(|ssh| (ssh.host.as_str(), ssh.port)),
        Some(("worker.example", 22))
    );

    let InteractiveWorkerEnsure::Reused(reused) = provider.ensure_worker(&request).expect("reuse interactive worker")
    else {
        panic!("existing worker must be reused");
    };
    assert_eq!(reused, status);

    let inspected = provider
        .inspect_worker(&status.worker)
        .expect("inspect interactive worker")
        .expect("worker present");
    assert!(inspected.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_eq!(
        provider.delete_worker(&status.worker),
        Ok(InteractiveWorkerCleanup::Deleted)
    );
    assert_eq!(
        provider.delete_worker(&status.worker),
        Ok(InteractiveWorkerCleanup::AlreadyAbsent)
    );

    let state = observer.0.lock().expect("state");
    assert_eq!(state.create_requests.len(), 1);
    assert!(
        state.create_requests[0]
            .env
            .iter()
            .any(|entry| entry.key == SSH_PUBLIC_KEY_ENV && entry.value == request.ssh_public_key)
    );
    assert_eq!(state.deleted, ["pod_123456"]);
    assert_eq!(
        *host_key_calls.lock().expect("host key calls"),
        [
            ("pod_123456".to_string(), "worker.example".to_string(), 22),
            ("pod_123456".to_string(), "worker.example".to_string(), 22),
            ("pod_123456".to_string(), "worker.example".to_string(), 22),
        ]
    );
}

#[test]
fn interactive_adapter_rejects_recovered_client_key_drift() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let request = interactive_request(workflow_id, job_id);
    let mut pod = running_pod(workflow_id, job_id, &request.target);
    pod.env.insert(SSH_PUBLIC_KEY_ENV.to_string(), ed25519_key(99));
    let transport = FakeTransport::with_pods(vec![pod]);
    let observer = transport.clone();
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(transport),
        profile(),
        FakeHostKeySource::new(Some(ed25519_key(73))),
    );

    assert_eq!(
        provider.ensure_worker(&request),
        Err(RunPodError::ResourceIdentityMismatch)
    );
    let state = observer.0.lock().expect("state");
    assert!(state.create_requests.is_empty());
    assert!(state.deleted.is_empty());
}

#[test]
fn interactive_adapter_waits_for_trusted_host_key_and_fails_closed_on_invalid_data() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let request = interactive_request(workflow_id, job_id);
    let pending_provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(FakeTransport::with_create_response(running_pod(
            workflow_id,
            job_id,
            &request.target,
        ))),
        profile(),
        FakeHostKeySource::new(None),
    );
    let pending = pending_provider
        .ensure_worker(&request)
        .expect("pending worker")
        .into_status();
    assert_eq!(pending.lifecycle, InteractiveWorkerLifecycle::Provisioning);
    assert!(pending.ssh.is_none());

    let invalid_key_provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(FakeTransport::with_create_response(running_pod(
            workflow_id,
            job_id,
            &request.target,
        ))),
        profile(),
        FakeHostKeySource::new(Some("ssh-rsa unsupported".to_string())),
    );
    let invalid_key = invalid_key_provider
        .ensure_worker(&request)
        .expect("worker identity remains recoverable")
        .into_status();
    assert_eq!(invalid_key.lifecycle, InteractiveWorkerLifecycle::Failed);
    assert!(invalid_key.ssh.is_none());

    let mut invalid_endpoint = running_pod(workflow_id, job_id, &request.target);
    invalid_endpoint
        .ssh
        .as_mut()
        .expect("SSH response")
        .direct
        .as_mut()
        .expect("direct SSH")
        .host = "-oProxyCommand=bad".to_string();
    let host_keys = FakeHostKeySource::new(Some(ed25519_key(73)));
    let host_key_calls = host_keys.calls.clone();
    let invalid_endpoint_provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(FakeTransport::with_create_response(invalid_endpoint)),
        profile(),
        host_keys,
    );
    let invalid_endpoint = invalid_endpoint_provider
        .ensure_worker(&request)
        .expect("worker identity remains recoverable")
        .into_status();
    assert_eq!(invalid_endpoint.lifecycle, InteractiveWorkerLifecycle::Failed);
    assert!(invalid_endpoint.ssh.is_none());
    assert!(host_key_calls.lock().expect("host key calls").is_empty());
}

#[test]
fn unsupported_persistent_requests_and_handles_fail_before_provider_io() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let mut request = interactive_request(workflow_id, job_id);
    request.target.lifetime = WorkerLifetime::Persistent;
    let transport = FakeTransport::default();
    let host_keys = FakeHostKeySource::new(None);
    let host_key_calls = host_keys.calls.clone();
    let provider =
        RunPodInteractiveWorkerProvider::new(RunPodClient::with_transport(transport.clone()), profile(), host_keys);
    assert!(request.is_valid_for(CloudProvider::RunPod));
    assert_eq!(provider.ensure_worker(&request), Err(RunPodError::InvalidTarget));
    let worker = InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: CloudProvider::RunPod,
            workflow_id,
            job_id,
            resource_id: "pod_123456".to_string(),
        },
        target: request.target,
        ssh_public_key: request.ssh_public_key,
        lifetime: InteractiveWorkerLifetime::Persistent,
    };
    assert!(worker.is_valid_for(CloudProvider::RunPod));
    assert_eq!(
        provider.inspect_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );
    assert_eq!(
        provider.delete_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );
    let state = transport.0.lock().expect("state");
    assert_eq!(state.list_calls, 0);
    assert!(state.create_requests.is_empty());
    assert!(state.inspected.is_empty());
    assert!(state.deleted.is_empty());
    assert!(host_key_calls.lock().expect("host key calls").is_empty());
}

#[test]
fn interactive_adapter_rejects_invalid_handles_before_transport_io() {
    let (workflow_id, job_id) = (CloudWorkflowId::new(), CloudJobId::new());
    let request = interactive_request(workflow_id, job_id);
    let transport = FakeTransport::default();
    let observer = transport.clone();
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(transport),
        profile(),
        FakeHostKeySource::new(None),
    );
    let mut invalid_request = request.clone();
    invalid_request.ssh_public_key = "ssh-rsa unsupported".to_string();
    assert_eq!(
        provider.ensure_worker(&invalid_request),
        Err(RunPodError::InvalidTarget)
    );
    let mut worker = InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: CloudProvider::Azure,
            workflow_id,
            job_id,
            resource_id: "pod_123456".to_string(),
        },
        target: request.target,
        ssh_public_key: request.ssh_public_key,
        lifetime: InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: termination_deadline(900).expect("termination deadline"),
        }),
    };

    assert_eq!(
        provider.inspect_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );
    assert_eq!(
        provider.delete_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );
    worker.identity.provider = CloudProvider::RunPod;
    worker.identity.resource_id = "x".repeat(192);
    assert_eq!(
        provider.inspect_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );
    assert_eq!(
        provider.delete_worker(&worker),
        Err(RunPodError::InvalidPersistedWorker)
    );

    let state = observer.0.lock().expect("state");
    assert!(state.inspected.is_empty());
    assert!(state.deleted.is_empty());
}
