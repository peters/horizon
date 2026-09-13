use super::*;
use crate::cloud_run::interactive_worker_delete::{
    InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation as Observation,
};
use serde_json::json;

struct Fixture {
    request: InteractiveWorkerRequest,
    worker: InteractiveWorker,
    transport: FakeTransport,
}

impl Fixture {
    fn new() -> Self {
        let request = persistence::persistent_request();
        let pod = persistence::persistent_pod(&request);
        let worker = InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: pod.id.clone(),
            },
            target: request.target.clone(),
            ssh_public_key: request.ssh_public_key.clone(),
            lifetime: InteractiveWorkerLifetime::Persistent,
        };
        Self {
            request,
            worker,
            transport: FakeTransport::with_pods(vec![pod]),
        }
    }

    fn client(&self) -> RunPodClient {
        RunPodClient::with_transport_and_fence(self.transport.clone(), |_, _, _: &WorkerTarget, _: &str| {
            panic!("deletion must not claim creation")
        })
    }

    fn provider(&self, selection: Option<&RunPodNetworkVolumeExpectation>) -> RunPodInteractiveWorkerProvider {
        let mut client = self.client();
        if let Some(selection) = selection {
            let binding = network_attachment::NetworkBinding::new(&self.request, selection, &profile())
                .expect("synthetic request binding");
            client.bind_network(&binding).expect("test-only binding");
        }
        RunPodInteractiveWorkerProvider::new(client, profile(), no_host_key)
    }

    fn deletion_provider(&self) -> RunPodDeletionWorkerProvider {
        RunPodDeletionWorkerProvider::new_with_network_volume(self.client(), profile(), &self.request, &selection())
            .expect("saved deletion binding without host pin")
    }

    fn pod(&self) -> ApiPod {
        self.transport.0.lock().expect("state").pods[0].clone()
    }

    fn assert_calls(&self, count: usize) {
        let state = self.transport.0.lock().expect("state");
        assert_eq!(state.inspected, vec![self.worker.identity.resource_id.clone(); count]);
        assert_eq!(state.list_calls, 0);
        assert!(state.create_requests.is_empty());
        assert!(state.deleted.is_empty());
        assert!(state.stopped.is_empty());
    }
}

fn no_host_key(_: &RunPodWorker, _: &RunPodSshEndpoint, _: &str) -> Option<String> {
    panic!("deletion observation must not consult a host-key source")
}

fn selection() -> RunPodNetworkVolumeExpectation {
    RunPodNetworkVolumeExpectation {
        volume_id: "volume_exact".into(),
        data_center_id: "EUR-NO-1".into(),
        minimum_size_gb: 10,
    }
}

fn network_metadata() -> serde_json::Value {
    json!({
        "cloud": "SECURE", "dataCenterId": "EUR-NO-1",
        "mounts": {"network": [{"volumeId": "volume_exact", "path": "/workspace"}]}
    })
}

#[test]
fn surviving_owned_pod_is_present_in_every_state_without_a_host_pin_or_endpoint() {
    for state in [
        None,
        Some("PROVISIONING"),
        Some("STARTING"),
        Some("RUNNING"),
        Some("EXITED"),
        Some("TERMINATED"),
        Some("ERROR"),
        Some("DELETING"),
        Some("unknown"),
    ] {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        pod.status = state.map(str::to_string);
        pod.ssh = None;
        pod.cost = None;
        fixture.transport.0.lock().expect("state").pods = vec![pod];
        assert_eq!(
            fixture.provider(None).observe_worker_deletion(&fixture.worker),
            Ok(Observation::Present)
        );
        fixture.assert_calls(1);
    }
}

#[test]
fn one_exact_read_is_authoritative_without_follow_up_or_name_adoption() {
    for absent in [false, true] {
        let fixture = Fixture::new();
        let pod = fixture.pod();
        fixture.transport.0.lock().expect("state").scripted_gets =
            vec![Ok((!absent).then_some(pod.clone())), Ok(absent.then_some(pod))];
        assert_eq!(
            fixture.provider(None).observe_worker_deletion(&fixture.worker),
            Ok(if absent {
                Observation::Absent
            } else {
                Observation::Present
            })
        );
        fixture.assert_calls(1);
        assert_eq!(fixture.transport.0.lock().expect("state").scripted_gets.len(), 1);
    }
}

#[test]
fn invalid_saved_handle_and_profile_fail_before_lookup() {
    for change in 0..9 {
        let fixture = Fixture::new();
        let mut worker = fixture.worker.clone();
        match change {
            0 => worker.identity.resource_id = "../other".into(),
            1 => worker.identity.provider = CloudProvider::Azure,
            2 => worker.target.provider = CloudProvider::Azure,
            3 => worker.target.image = "example/worker:latest".into(),
            4 => worker.target.profile = "different".into(),
            5 => worker.ssh_public_key.clear(),
            6 => worker.target.lifetime = WorkerLifetime::TimeLimited { seconds: 300 },
            7 => worker.target.disk_gib = 0,
            _ => worker.target.max_hourly_cost_micros = Some(0),
        }
        assert!(fixture.provider(None).observe_worker_deletion(&worker).is_err());
        fixture.assert_calls(0);
    }
    let fixture = Fixture::new();
    let mut invalid = profile();
    invalid.gpu_count = 0;
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(fixture.transport.clone()),
        invalid,
        no_host_key,
    );
    assert_eq!(
        provider.observe_worker_deletion(&fixture.worker),
        Err(RunPodError::InvalidTarget)
    );
    fixture.assert_calls(0);
}

#[test]
fn mismatched_or_missing_returned_ownership_is_not_presence_or_absence() {
    for change in 0..3 {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        match change {
            0 => pod.id = "foreign_pod".into(),
            1 => pod.name = "foreign_name".into(),
            _ => pod.image = format!("example/other@sha256:{}", "e".repeat(64)),
        }
        fixture.transport.0.lock().expect("state").scripted_gets = vec![Ok(Some(pod))];
        assert_eq!(
            fixture.provider(None).observe_worker_deletion(&fixture.worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        fixture.assert_calls(1);
    }
    for field in [WORKFLOW_ENV, JOB_ENV, PROTOCOL_ENV, SSH_PUBLIC_KEY_ENV, LIFETIME_ENV] {
        for missing in [false, true] {
            let fixture = Fixture::new();
            let mut pod = fixture.pod();
            if missing {
                pod.env.remove(field);
            } else {
                pod.env.insert(field.into(), "different".into());
            }
            fixture.transport.0.lock().expect("state").pods = vec![pod];
            assert_eq!(
                fixture.provider(None).observe_worker_deletion(&fixture.worker),
                Err(RunPodError::ResourceIdentityMismatch)
            );
            fixture.assert_calls(1);
        }
    }
}

#[test]
fn failed_reads_are_errors_and_do_not_retry_even_when_later_absence_is_available() {
    let operation = "pod inspection";
    let mut errors: Vec<_> = [401, 403, 404, 408, 429, 500, 503]
        .into_iter()
        .map(|status| RunPodError::UnexpectedStatus { operation, status })
        .collect();
    errors.extend([
        RunPodError::RequestFailed { operation },
        RunPodError::InvalidResponse { operation },
    ]);
    for error in errors {
        let fixture = Fixture::new();
        let expected = error.to_string();
        fixture.transport.0.lock().expect("state").scripted_gets = vec![Err(error), Ok(None)];
        let actual = fixture
            .provider(None)
            .observe_worker_deletion(&fixture.worker)
            .expect_err("read remains unverified");
        assert_eq!(actual.to_string(), expected);
        fixture.assert_calls(1);
        assert_eq!(fixture.transport.0.lock().expect("state").scripted_gets.len(), 1);
    }
}

#[test]
fn bound_network_observation_reads_only_the_pod_without_proving_volume_state() {
    for absent in [false, true] {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        pod.ssh = None;
        pod.stop = serde_json::from_value(network_metadata()).expect("metadata");
        fixture.transport.0.lock().expect("state").pods = if absent { vec![] } else { vec![pod] };
        assert_eq!(
            fixture
                .provider(Some(&selection()))
                .observe_worker_deletion(&fixture.worker),
            Ok(if absent {
                Observation::Absent
            } else {
                Observation::Present
            })
        );
        fixture.assert_calls(1);
    }
}

#[test]
fn bound_request_drift_fails_before_reading_the_pod() {
    for change in 0..4 {
        let fixture = Fixture::new();
        let mut worker = fixture.worker.clone();
        match change {
            0 => worker.identity.workflow_id = CloudWorkflowId::new(),
            1 => worker.identity.job_id = CloudJobId::new(),
            2 => worker.target.max_hourly_cost_micros = Some(999_999),
            _ => worker.ssh_public_key = ed25519_key(99),
        }
        assert_eq!(
            fixture.provider(Some(&selection())).observe_worker_deletion(&worker),
            Err(RunPodError::InvalidTarget)
        );
        fixture.assert_calls(0);
    }
}

#[test]
fn mismatched_or_unbound_network_attachment_is_not_adopted() {
    for change in 0..7 {
        let fixture = Fixture::new();
        let mut metadata = network_metadata();
        match change {
            0 => metadata["mounts"]["network"][0]["volumeId"] = json!("foreign"),
            1 => metadata["mounts"]["network"][0]["path"] = json!("/other"),
            2 => metadata["dataCenterId"] = json!("different"),
            3 => metadata["cloud"] = json!("COMMUNITY"),
            4 => metadata["mounts"]["network"] = json!([]),
            5 => metadata["mounts"] = json!({"persistent": {"size": 10, "path": "/workspace"}}),
            _ => {}
        }
        let mut pod = fixture.pod();
        pod.stop = serde_json::from_value(metadata).expect("metadata");
        fixture.transport.0.lock().expect("state").pods = vec![pod];
        assert_eq!(
            fixture
                .provider((change != 6).then_some(&selection()))
                .observe_worker_deletion(&fixture.worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        fixture.assert_calls(1);
    }
}

#[test]
fn expired_worker_and_changed_cost_do_not_trigger_lifecycle_cleanup() {
    let fixture = Fixture::new();
    let deadline = "2000-01-01T00:00:00Z";
    let mut worker = fixture.worker.clone();
    worker.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3_600 };
    worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: deadline.into(),
    });
    let mut pod = fixture.pod();
    pod.env.remove(LIFETIME_ENV);
    pod.env.insert(TERMINATE_ENV.into(), deadline.into());
    pod.cost = Some(u64::MAX);
    fixture.transport.0.lock().expect("state").pods = vec![pod];
    assert_eq!(
        fixture.provider(None).observe_worker_deletion(&worker),
        Ok(Observation::Present)
    );
    fixture.assert_calls(1);
}

#[test]
fn deletion_only_public_constructor_and_observer_need_no_ssh_pin_or_volume_read() {
    for state in [None, Some("RUNNING"), Some("TERMINATED")] {
        for absent in [false, true] {
            let fixture = Fixture::new();
            let mut pod = fixture.pod();
            pod.ssh = None;
            pod.status = state.map(str::to_string);
            pod.stop = serde_json::from_value(network_metadata()).expect("network metadata");
            fixture.transport.0.lock().expect("state").pods = if absent { vec![] } else { vec![pod] };
            let provider = fixture.deletion_provider();
            fixture.assert_calls(0);
            assert_eq!(provider.provider(), CloudProvider::RunPod);
            assert_eq!(
                provider.observe_worker_deletion(&fixture.worker),
                Ok(if absent {
                    Observation::Absent
                } else {
                    Observation::Present
                })
            );
            fixture.assert_calls(1);
        }
    }
}

#[test]
fn deletion_only_provider_refuses_provisioning_recovery_and_readiness_before_io() {
    let fixture = Fixture::new();
    let provider = fixture.deletion_provider();
    assert_eq!(
        provider.ensure_worker(&fixture.request),
        Err(RunPodError::InvalidTarget)
    );
    assert_eq!(
        provider.reconcile_worker(&fixture.request),
        Err(RunPodError::InvalidTarget)
    );
    assert_eq!(
        provider.inspect_worker(&fixture.worker),
        Err(RunPodError::InvalidTarget)
    );
    fixture.assert_calls(0);
}

#[test]
fn deletion_constructor_rejects_invalid_or_prebound_conflicting_selections_before_io() {
    for change in 0..12 {
        let fixture = Fixture::new();
        let mut expected = fixture.request.clone();
        let mut selected = selection();
        let mut selected_profile = profile();
        let mut client = fixture.client();
        match change {
            0 => selected_profile.gpu_count = 0,
            1 => selected_profile.name = "other".into(),
            2 => selected.volume_id = "../foreign".into(),
            3 => selected.data_center_id = "other-center".into(),
            4 => selected.minimum_size_gb = 0,
            5 => expected.ssh_public_key.clear(),
            6 => expected.target.image = "example/mutable:latest".into(),
            7 => expected.target.provider = CloudProvider::Azure,
            8 => expected.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3_600 },
            9 => expected.target.disk_gib = 0,
            _ => {
                let mut prior_request = expected.clone();
                let mut prior_selection = selected.clone();
                if change == 10 {
                    prior_request.job_id = CloudJobId::new();
                } else {
                    prior_selection.minimum_size_gb = 20;
                }
                let prior =
                    network_attachment::NetworkBinding::new(&prior_request, &prior_selection, &selected_profile)
                        .expect("prior binding");
                client.bind_network(&prior).expect("bind once");
            }
        }
        assert!(matches!(
            RunPodDeletionWorkerProvider::new_with_network_volume(client, selected_profile, &expected, &selected),
            Err(RunPodError::InvalidTarget)
        ));
        fixture.assert_calls(0);
    }
    let fixture = Fixture::new();
    let mut client = fixture.client();
    let binding = network_attachment::NetworkBinding::new(&fixture.request, &selection(), &profile()).expect("binding");
    client.bind_network(&binding).expect("initial binding");
    assert!(
        RunPodDeletionWorkerProvider::new_with_network_volume(client, profile(), &fixture.request, &selection())
            .is_ok()
    );
    fixture.assert_calls(0);
}

#[test]
fn deletion_wrapper_keeps_the_full_saved_request_fence_for_observation_and_delete() {
    for change in 0..7 {
        let fixture = Fixture::new();
        let provider = fixture.deletion_provider();
        let mut worker = fixture.worker.clone();
        match change {
            0 => worker.identity.workflow_id = CloudWorkflowId::new(),
            1 => worker.identity.job_id = CloudJobId::new(),
            2 => worker.target.disk_gib += 1,
            3 => worker.target.max_hourly_cost_micros = Some(999_999),
            4 => worker.ssh_public_key = ed25519_key(99),
            5 => worker.target.profile = "other".into(),
            _ => worker.identity.provider = CloudProvider::Azure,
        }
        assert!(provider.observe_worker_deletion(&worker).is_err());
        assert!(provider.delete_worker(&worker).is_err());
        fixture.assert_calls(0);
    }
}

struct CleanupTransport {
    pods: FakeTransport,
    volume: Option<serde_json::Value>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl Transport for CleanupTransport {
    fn network_volume(&self, id: &str) -> Result<Option<network_volume::ApiNetworkVolume>, RunPodError> {
        assert_eq!(id, "volume_exact");
        self.calls.lock().expect("calls").push("volume");
        Ok(self
            .volume
            .clone()
            .map(|value| serde_json::from_value(value).expect("volume")))
    }

    fn get(&self, id: &str) -> Result<Option<ApiPod>, RunPodError> {
        self.calls.lock().expect("calls").push("get");
        self.pods.get(id)
    }

    fn delete(&self, id: &str) -> Result<RunPodCleanup, RunPodError> {
        self.calls.lock().expect("calls").push("delete");
        self.pods.delete(id)
    }

    fn list_by_name(&self, _: &str) -> Result<Vec<ApiPod>, RunPodError> {
        panic!("deletion-only provider must not reconcile names")
    }

    fn create(&self, _: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        panic!("deletion-only provider must not create")
    }

    fn stop(&self, _: &str) -> Result<(), RunPodError> {
        panic!("deletion-only provider must not stop")
    }
}

#[test]
fn deletion_without_a_pin_retains_the_existing_volume_and_pod_admission() {
    for case in 0..5 {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        pod.ssh = None;
        pod.stop = serde_json::from_value(network_metadata()).expect("network metadata");
        if case == 4 {
            pod.name = "foreign".into();
        }
        fixture.transport.0.lock().expect("state").pods = if case == 1 { vec![] } else { vec![pod] };
        let mut volume =
            json!({"id": "volume_exact", "dataCenter": "EUR-NO-1", "size": 10, "type": "HIGH_PERFORMANCE"});
        if case == 3 {
            volume["id"] = json!("foreign");
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let transport = CleanupTransport {
            pods: fixture.transport.clone(),
            volume: (case != 2).then_some(volume),
            calls: calls.clone(),
        };
        let client =
            RunPodClient::with_transport_and_fence(transport, |_, _, _: &WorkerTarget, _: &str| panic!("no claim"));
        let provider =
            RunPodDeletionWorkerProvider::new_with_network_volume(client, profile(), &fixture.request, &selection())
                .expect("deletion binding");
        assert!(calls.lock().expect("calls").is_empty());
        let result = provider.delete_worker(&fixture.worker);
        let expected = match case {
            0 => Ok(InteractiveWorkerCleanup::Deleted),
            1 => Ok(InteractiveWorkerCleanup::AlreadyAbsent),
            _ => Err(RunPodError::ResourceIdentityMismatch),
        };
        assert_eq!(result, expected);
        let expected_calls: &[&str] = match case {
            0 => &["volume", "get", "delete"],
            1 | 4 => &["volume", "get"],
            _ => &["volume"],
        };
        assert_eq!(*calls.lock().expect("calls"), expected_calls);
        let state = fixture.transport.0.lock().expect("state");
        assert_eq!(
            state.deleted,
            if case == 0 {
                vec![fixture.worker.identity.resource_id.clone()]
            } else {
                vec![]
            }
        );
        assert_eq!(state.list_calls, 0);
        assert!(state.create_requests.is_empty() && state.stopped.is_empty());
    }
}
