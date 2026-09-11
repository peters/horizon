use super::super::{
    RunPodApiKey, RunPodCleanup, RunPodHostTrust, RunPodInteractiveWorkerProvider, Transport,
    interactive::runpod_worker,
    network_volume::ApiNetworkVolume,
    resource_name,
    stop::StopMetadata,
    tests::{ed25519_key, interactive_request, profile},
};
use super::*;
use crate::cloud_run::interactive_worker::{
    InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity, InteractiveWorkerLifetime,
    InteractiveWorkerProvider, InteractiveWorkerSshEndpoint,
};
use crate::cloud_run::interactive_worker_stop::InteractiveWorkerStopProvider;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct FakeTransport(Arc<Mutex<State>>);

struct State {
    calls: Vec<&'static str>,
    volume: Option<Value>,
    pods: Vec<ApiPod>,
    template: ApiPod,
    creates: Vec<CreatePodRequest>,
    lose_create: bool,
    fresh: Option<ApiPod>,
    deleted: Vec<String>,
}

impl Transport for FakeTransport {
    fn network_volume(&self, id: &str) -> Result<Option<ApiNetworkVolume>, RunPodError> {
        assert_eq!(id, "volume_exact");
        let mut state = self.0.lock().expect("state");
        state.calls.push("volume");
        Ok(state
            .volume
            .clone()
            .map(|value| serde_json::from_value(value).expect("volume")))
    }
    fn list_by_name(&self, _: &str) -> Result<Vec<ApiPod>, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.calls.push("list");
        Ok(state.pods.clone())
    }
    fn create(&self, request: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.calls.push("create");
        state.creates.push(request.clone());
        let pod = state.template.clone();
        state.pods.push(pod.clone());
        if state.lose_create {
            Err(RunPodError::RequestFailed {
                operation: "pod creation",
            })
        } else {
            Ok(pod)
        }
    }
    fn get(&self, id: &str) -> Result<Option<ApiPod>, RunPodError> {
        assert_eq!(id, "pod_exact");
        let mut state = self.0.lock().expect("state");
        state.calls.push("get");
        Ok(state.fresh.clone().or_else(|| state.pods.first().cloned()))
    }
    fn stop(&self, _: &str) -> Result<(), RunPodError> {
        panic!("network Stop is unsupported")
    }
    fn delete(&self, id: &str) -> Result<RunPodCleanup, RunPodError> {
        let mut state = self.0.lock().expect("state");
        state.calls.push("delete");
        state.deleted.push(id.into());
        state.pods.clear();
        Ok(RunPodCleanup::Deleted)
    }
}

struct Fixture {
    request: InteractiveWorkerRequest,
    selection: RunPodNetworkVolumeExpectation,
    transport: FakeTransport,
    claims: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
        request.target.lifetime = WorkerLifetime::Persistent;
        let selection = RunPodNetworkVolumeExpectation {
            volume_id: "volume_exact".into(),
            data_center_id: "EUR-NO-1".into(),
            minimum_size_gb: 10,
        };
        let template = serde_json::from_value(json!({
            "id": "pod_exact", "name": resource_name(request.workflow_id, request.job_id),
            "image": request.target.image, "status": "RUNNING", "cost": 0.5,
            "env": {"HORIZON_WORKFLOW_ID": request.workflow_id, "HORIZON_JOB_ID": request.job_id,
                "HORIZON_CLOUD_PROTOCOL_VERSION": "1", "HORIZON_WORKER_LIFETIME": "persistent",
                "HORIZON_SSH_PUBLIC_KEY": request.ssh_public_key},
            "ssh": {"direct": {"username": "root", "host": "worker.example", "port": 2200}},
            "cloud": "SECURE", "dataCenterId": selection.data_center_id,
            "mounts": {"network": [{"volumeId": selection.volume_id, "path": "/workspace"}]}
        }))
        .expect("pod");
        let volume = Some(
            json!({"id": selection.volume_id, "dataCenter": selection.data_center_id,
            "size": 10, "type": "HIGH_PERFORMANCE"}),
        );
        Self {
            request,
            selection,
            claims: Arc::new(AtomicUsize::new(0)),
            transport: FakeTransport(Arc::new(Mutex::new(State {
                calls: Vec::new(),
                volume,
                pods: Vec::new(),
                template,
                creates: Vec::new(),
                lose_create: false,
                fresh: None,
                deleted: Vec::new(),
            }))),
        }
    }
    fn client(&self) -> RunPodClient {
        let claims = self.claims.clone();
        let transport = self.transport.clone();
        RunPodClient::with_transport_and_fence(self.transport.clone(), move |_, _, _: &WorkerTarget, _: &str| {
            transport.0.lock().expect("state").calls.push("claim");
            Ok(claims.fetch_add(1, Ordering::SeqCst) == 0)
        })
    }
    fn worker(&self) -> InteractiveWorker {
        InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: self.request.workflow_id,
                job_id: self.request.job_id,
                resource_id: "pod_exact".into(),
            },
            target: self.request.target.clone(),
            ssh_public_key: self.request.ssh_public_key.clone(),
            lifetime: InteractiveWorkerLifetime::Persistent,
        }
    }
    fn trust(&self) -> RunPodHostTrust {
        RunPodHostTrust::retained(
            &self.worker(),
            &InteractiveWorkerSshEndpoint {
                username: "root".into(),
                host: "worker.example".into(),
                port: 2200,
                host_key: ed25519_key(73),
            },
        )
        .expect("retained trust")
    }
    fn provider(&self) -> RunPodInteractiveWorkerProvider {
        RunPodInteractiveWorkerProvider::new_with_network_volume(
            self.client(),
            profile(),
            self.trust(),
            &self.request,
            &self.selection,
        )
        .expect("bound provider")
    }
    fn existing(&self) {
        let mut state = self.transport.0.lock().expect("state");
        state.pods = vec![state.template.clone()];
    }
    fn untouched(&self) {
        assert!(self.transport.0.lock().expect("state").calls.is_empty());
        assert_eq!(self.claims.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn constructor_refuses_conflicts_and_never_silently_rebinds() {
    let fixture = Fixture::new();
    let binding = NetworkBinding::new(&fixture.request, &fixture.selection, &profile()).expect("binding");
    for change in 0..6 {
        let mut selection = fixture.selection.clone();
        let mut expected = fixture.request.clone();
        let mut selected_profile = profile();
        match change {
            0 => selection.volume_id = "../wrong".into(),
            1 => selection.minimum_size_gb = 0,
            2 => selected_profile.data_center_id = Some("other".into()),
            3 => expected.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3600 },
            4 => expected.job_id = CloudJobId::new(),
            _ => selection.volume_id = "another_volume".into(),
        }
        let mut client = fixture.client();
        client.bind_network(&binding).expect("first bind");
        assert!(
            RunPodInteractiveWorkerProvider::new_with_network_volume(
                client,
                selected_profile,
                fixture.trust(),
                &expected,
                &selection
            )
            .is_err()
        );
    }
    let mut trust = fixture.trust();
    trust.bind_network(&binding).expect("first trust binding");
    let mut changed = binding.clone();
    changed.selection.minimum_size_gb += 1;
    assert_eq!(trust.bind_network(&changed), Err(RunPodError::InvalidTarget));
    trust.bind_network(&binding).expect("same trust binding");
    let api_key = RunPodApiKey::new("synthetic").expect("key");
    let mut other = fixture.request.clone();
    other.job_id = CloudJobId::new();
    let trust = RunPodHostTrust::initial_task_free(&api_key, &other).expect("other trust");
    assert!(
        RunPodInteractiveWorkerProvider::new_with_network_volume(
            fixture.client(),
            profile(),
            trust,
            &fixture.request,
            &fixture.selection
        )
        .is_err()
    );
    fixture.untouched();
}

#[test]
fn complete_request_and_original_worker_guards_precede_every_io_path() {
    let fixture = Fixture::new();
    let provider = fixture.provider();
    for change in 0..9 {
        let mut request = fixture.request.clone();
        match change {
            0 => request.workflow_id = CloudWorkflowId::new(),
            1 => request.job_id = CloudJobId::new(),
            2 => request.target.profile = "other".into(),
            3 => request.target.disk_gib += 1,
            4 => request.target.max_hourly_cost_micros = Some(123),
            5 => request.target.image = request.target.image.replace('d', "e"),
            6 => request.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3600 },
            7 => request.ssh_public_key = ed25519_key(90),
            _ => request.target.provider = CloudProvider::Azure,
        }
        assert!(provider.ensure_worker(&request).is_err());
        assert!(provider.reconcile_worker(&request).is_err());
        let mut worker = fixture.worker();
        worker.identity.workflow_id = request.workflow_id;
        worker.identity.job_id = request.job_id;
        worker.target = request.target;
        worker.ssh_public_key = request.ssh_public_key;
        assert!(provider.inspect_worker(&worker).is_err());
        assert!(provider.delete_worker(&worker).is_err());
        assert!(provider.stop_worker(&worker).is_err());
    }
    assert_eq!(
        provider.stop_worker(&fixture.worker()),
        Err(RunPodError::StopRetentionUnverified)
    );
    let mut client = fixture.client();
    client
        .bind_network(&NetworkBinding::new(&fixture.request, &fixture.selection, &profile()).expect("binding"))
        .expect("bind");
    let worker = runpod_worker(&fixture.worker()).expect("worker");
    assert!(client.inspect_worker(&worker).is_err());
    assert!(client.delete_worker(&worker).is_err());
    assert!(
        client
            .ensure_worker(
                fixture.request.workflow_id,
                fixture.request.job_id,
                &fixture.request.target,
                &profile()
            )
            .is_err()
    );
    fixture.untouched();
}

#[test]
fn absent_or_mismatched_fresh_volume_never_consumes_a_creation_claim() {
    for change in 0..5 {
        let fixture = Fixture::new();
        {
            let mut state = fixture.transport.0.lock().expect("state");
            let volume = state.volume.as_mut().expect("volume");
            match change {
                0 => volume["id"] = json!("other"),
                1 => volume["dataCenter"] = json!("other"),
                2 => volume["type"] = json!("STANDARD"),
                3 => volume["size"] = json!(9),
                _ => state.volume = None,
            }
        }
        assert_eq!(
            fixture.provider().ensure_worker(&fixture.request),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_eq!(fixture.claims.load(Ordering::SeqCst), 0);
        let state = fixture.transport.0.lock().expect("state");
        assert!(state.creates.is_empty() && state.deleted.is_empty());
    }
}

#[test]
fn selected_creation_uses_exact_payload_and_checks_metadata_before_claim() {
    let fixture = Fixture::new();
    let mut selected_profile = profile();
    selected_profile.data_center_id = None;
    let provider = RunPodInteractiveWorkerProvider::new_with_network_volume(
        fixture.client(),
        selected_profile,
        fixture.trust(),
        &fixture.request,
        &fixture.selection,
    )
    .expect("bind");
    let InteractiveWorkerEnsure::Created(status) = provider.ensure_worker(&fixture.request).expect("create") else {
        panic!("created")
    };
    assert!(status.is_ready_for(&fixture.request, time::OffsetDateTime::now_utc()));
    let state = fixture.transport.0.lock().expect("state");
    let payload = serde_json::to_value(&state.creates[0]).expect("payload");
    assert_eq!(payload["networkVolumeId"], "volume_exact");
    assert_eq!(payload["dataCenterId"], "EUR-NO-1");
    assert_eq!(payload["volumeInGb"], 0);
    assert_eq!(payload["volumeMountPath"], "/workspace");
    assert_eq!(payload["cloudType"], "SECURE");
    assert!(payload.get("terminateAfter").is_none());
    assert!(
        state
            .calls
            .windows(3)
            .any(|calls| calls == ["volume", "claim", "create"])
    );
    assert_eq!(state.calls.last(), Some(&"get"));
}

#[test]
fn lost_create_response_is_recovered_without_another_create_or_cleanup() {
    let fixture = Fixture::new();
    fixture.transport.0.lock().expect("state").lose_create = true;
    assert!(matches!(
        fixture.provider().ensure_worker(&fixture.request),
        Err(RunPodError::PersistentCreationUnresolved { .. })
    ));
    let pod = fixture
        .transport
        .0
        .lock()
        .expect("state")
        .pods
        .pop()
        .expect("created pod");
    assert!(matches!(
        fixture.provider().ensure_worker(&fixture.request),
        Err(RunPodError::CreationUnresolved { .. })
    ));
    fixture.transport.0.lock().expect("state").pods.push(pod);
    let recovered = fixture
        .provider()
        .reconcile_worker(&fixture.request)
        .expect("recover")
        .expect("pod");
    assert!(recovered.is_ready_for(&fixture.request, time::OffsetDateTime::now_utc()));
    assert!(matches!(
        fixture.provider().ensure_worker(&fixture.request),
        Ok(InteractiveWorkerEnsure::Reused(_))
    ));
    let state = fixture.transport.0.lock().expect("state");
    assert_eq!(state.creates.len(), 1);
    assert!(state.deleted.is_empty());
}

#[test]
fn v2_attachment_mismatches_never_adopt_ready_or_delete_a_resource() {
    for metadata in [
        json!({}),
        json!({"network": []}),
        json!({"network": [{"volumeId":"other","path":"/workspace"}]}),
        json!({"network": [{"volumeId":"volume_exact","path":"/elsewhere"}]}),
        json!({"network": [{"volumeId":"volume_exact","path":"/workspace"},{"volumeId":"volume_exact","path":"/workspace"}]}),
        json!({"network": [{"volumeId":"volume_exact","path":"/workspace"}],"persistent":{"path":"/workspace","size":10}}),
    ] {
        let fixture = Fixture::new();
        fixture.existing();
        fixture.transport.0.lock().expect("state").pods[0].stop = serde_json::from_value(json!({
            "mounts": metadata, "cloud":"SECURE", "dataCenterId":"EUR-NO-1"}))
        .expect("metadata");
        let provider = fixture.provider();
        assert!(provider.ensure_worker(&fixture.request).is_err());
        assert!(provider.reconcile_worker(&fixture.request).is_err());
        assert!(provider.inspect_worker(&fixture.worker()).is_err());
        assert!(provider.delete_worker(&fixture.worker()).is_err());
        let state = fixture.transport.0.lock().expect("state");
        assert!(state.creates.is_empty() && state.deleted.is_empty());
        assert_eq!(fixture.claims.load(Ordering::SeqCst), 0);
    }
    for field in ["cloud", "dataCenterId"] {
        let fixture = Fixture::new();
        let mut metadata = json!({"mounts":{"network":[{"volumeId":"volume_exact","path":"/workspace"}]},
            "cloud":"SECURE", "dataCenterId":"EUR-NO-1"});
        metadata[field] = json!("other");
        let mut state = fixture.transport.0.lock().expect("state");
        state.template.stop = serde_json::from_value(metadata).expect("metadata");
        drop(state);
        assert!(fixture.provider().ensure_worker(&fixture.request).is_err());
        assert!(fixture.transport.0.lock().expect("state").deleted.is_empty());
    }
}

#[test]
fn fresh_exact_pod_and_ordinary_rejection_cannot_be_bypassed() {
    let fixture = Fixture::new();
    fixture.existing();
    let mut drift = fixture.transport.0.lock().expect("state").template.clone();
    drift.stop = StopMetadata::default();
    fixture.transport.0.lock().expect("state").fresh = Some(drift);
    assert!(fixture.provider().ensure_worker(&fixture.request).is_err());
    fixture.transport.0.lock().expect("state").fresh = None;
    let ordinary = RunPodInteractiveWorkerProvider::new(fixture.client(), profile(), fixture.trust());
    assert!(ordinary.ensure_worker(&fixture.request).is_err());
    assert!(ordinary.reconcile_worker(&fixture.request).is_err());
    assert!(ordinary.inspect_worker(&fixture.worker()).is_err());
    assert!(ordinary.delete_worker(&fixture.worker()).is_err());
    let worker = runpod_worker(&fixture.worker()).expect("worker");
    assert!(fixture.client().inspect_worker(&worker).is_err());
    assert!(fixture.client().delete_worker(&worker).is_err());
    assert!(fixture.transport.0.lock().expect("state").deleted.is_empty());
    assert_eq!(
        fixture.provider().delete_worker(&fixture.worker()),
        Ok(InteractiveWorkerCleanup::Deleted)
    );
    assert_eq!(fixture.transport.0.lock().expect("state").deleted, ["pod_exact"]);
}
