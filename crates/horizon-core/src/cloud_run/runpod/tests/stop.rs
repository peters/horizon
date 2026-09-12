use super::{persistence::persistent_pod, persistence::persistent_request, *};
use crate::{
    cloud_run::{
        CloudWorkflowStore, StoredRemoteAllocation,
        interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider},
    },
    remote_workspace::stop::{RemoteWorkspaceStopError, stop_remote_workspace},
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
};
use serde_json::{Value, json};

mod confirmation;

struct Fixture {
    worker: InteractiveWorker,
    transport: FakeTransport,
    keys: FakeHostKeySource,
}

impl Fixture {
    fn new() -> Self {
        Self::from_request(persistent_request())
    }

    fn from_request(request: InteractiveWorkerRequest) -> Self {
        let mut pod = persistent_pod(&request);
        pod.stop = serde_json::from_value(metadata()).expect("retention metadata");
        Self {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::RunPod,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: pod.id.clone(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            transport: FakeTransport::with_pods(vec![pod]),
            keys: FakeHostKeySource::new(None),
        }
    }

    fn provider(&self) -> RunPodInteractiveWorkerProvider {
        RunPodInteractiveWorkerProvider::new(
            RunPodClient::with_transport(self.transport.clone()),
            profile(),
            self.keys.clone(),
        )
    }

    fn pod(&self) -> ApiPod {
        self.transport.0.lock().expect("state").pods[0].clone()
    }

    fn assert_calls(&self, reads: usize, stops: usize) {
        let state = self.transport.0.lock().expect("state");
        assert_eq!(state.inspected, vec![self.worker.identity.resource_id.clone(); reads]);
        assert_eq!(state.stopped, vec![self.worker.identity.resource_id.clone(); stops]);
        assert_eq!(state.list_calls, 0);
        assert!(state.create_requests.is_empty());
        assert!(state.deleted.is_empty());
        assert!(self.keys.calls.lock().expect("keys").is_empty());
    }
}

fn metadata() -> Value {
    json!({"mounts":{"persistent":{"size":20,"path":"/workspace"}},
        "cloud":"SECURE", "cluster":null, "locked":false,
        "actions":["stop","restart","terminate"], "runtime":null})
}

fn stopping_failure() -> RunPodError {
    RunPodError::RequestFailed { operation: "pod Stop" }
}

#[test]
fn explicit_stop_and_fresh_retry_retain_worker_without_keys_or_other_provider_operations() {
    let fixture = Fixture::new();
    let before = fixture.pod();
    assert_eq!(
        fixture.provider().stop_worker(&fixture.worker),
        Ok(InteractiveWorkerStop::Stopped)
    );
    fixture.assert_calls(2, 1);
    let retained = fixture.pod();
    assert_eq!(retained.id, before.id);
    assert_eq!(retained.env, before.env);
    assert_eq!(retained.status.as_deref(), Some("EXITED"));
    assert_eq!(
        fixture.provider().stop_worker(&fixture.worker),
        Ok(InteractiveWorkerStop::Stopped)
    );
    fixture.assert_calls(3, 1);
}

#[test]
fn stop_validates_worker_lifetime_and_profile_before_io() {
    for changed in 0..6 {
        let fixture = Fixture::new();
        let mut worker = fixture.worker.clone();
        let expected = match changed {
            0 => {
                worker.identity.resource_id = "../foreign".into();
                RunPodError::InvalidPersistedWorker
            }
            1 => {
                worker.identity.provider = CloudProvider::LocalDocker;
                RunPodError::InvalidPersistedWorker
            }
            2 => {
                worker.ssh_public_key.clear();
                RunPodError::InvalidPersistedWorker
            }
            3 => {
                worker.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
                worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                    terminate_after: termination_deadline(900).expect("deadline"),
                });
                RunPodError::StopUnsupportedLifetime
            }
            4 => {
                worker.target.profile = "other-profile".into();
                RunPodError::InvalidTarget
            }
            _ => {
                worker.target.image = "example/worker:latest".into();
                RunPodError::InvalidPersistedWorker
            }
        };
        assert_eq!(fixture.provider().stop_worker(&worker), Err(expected));
        fixture.assert_calls(0, 0);
    }
    let fixture = Fixture::new();
    let mut configuration = profile();
    configuration.volume_gib = 0;
    let provider = RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(fixture.transport.clone()),
        configuration,
        fixture.keys.clone(),
    );
    assert_eq!(
        provider.stop_worker(&fixture.worker),
        Err(RunPodError::StopRetentionUnverified)
    );
    fixture.assert_calls(0, 0);
}

#[test]
fn stop_rejects_missing_ambiguous_or_unowned_retention_before_mutation() {
    for changed in 0..12 {
        let fixture = Fixture::new();
        let mut data = metadata();
        match changed {
            0 => {
                data.as_object_mut().expect("metadata").remove("mounts");
            }
            1 => data["mounts"] = json!({}),
            2 => data["mounts"]["persistent"]["size"] = json!(0),
            3 => data["mounts"]["persistent"]["size"] = json!(19),
            4 => data["mounts"]["persistent"]["path"] = json!("/workspace/../private"),
            5 => data["mounts"] = json!({"network":[{"volumeId":"volume","path":"/workspace"}]}),
            6 => data["mounts"]["network"] = json!([]),
            7 => data["mounts"]["other"] = json!({"path":"/workspace"}),
            8 => data["mounts"]["persistent"]["size"] = json!(20.5),
            9 => data["cloud"] = json!("COMMUNITY"),
            10 => {
                data.as_object_mut().expect("metadata").remove("cloud");
            }
            _ => data["cluster"] = json!({"id":"foreign-cluster"}),
        }
        fixture.transport.0.lock().expect("state").pods[0].stop = serde_json::from_value(data).expect("raw metadata");
        assert_eq!(
            fixture.provider().stop_worker(&fixture.worker),
            Err(RunPodError::StopRetentionUnverified)
        );
        fixture.assert_calls(1, 0);
    }
}

#[test]
fn provisioning_stop_requires_unlocked_capability_and_retained_volume() {
    for status in ["PROVISIONING", "STARTING"] {
        for rejected in 0..4 {
            let fixture = Fixture::new();
            let mut pod = fixture.pod();
            pod.status = Some(status.into());
            let mut data = metadata();
            match rejected {
                1 => data["locked"] = json!(true),
                2 => data["actions"] = json!(["terminate"]),
                3 => data["mounts"]["persistent"]["size"] = json!(0),
                _ => {}
            }
            pod.stop = serde_json::from_value(data).expect("raw metadata");
            fixture.transport.0.lock().expect("state").pods = vec![pod];
            let expected = match rejected {
                0 => Ok(InteractiveWorkerStop::Stopped),
                3 => Err(RunPodError::StopRetentionUnverified),
                _ => Err(RunPodError::StopStateUnverified),
            };
            assert_eq!(fixture.provider().stop_worker(&fixture.worker), expected);
            if rejected == 0 {
                fixture.assert_calls(2, 1);
            } else {
                fixture.assert_calls(1, 0);
            }
        }
    }
}

#[test]
fn stop_rejects_unknown_states_capabilities_and_conflicting_live_state() {
    for changed in 0..8 {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        let mut data = metadata();
        match changed {
            0 => pod.status = None,
            1 => pod.status = Some("TERMINATED".into()),
            2 => pod.status = Some("ERROR".into()),
            3 => pod.status = Some("UNRECOGNIZED".into()),
            4 => data["locked"] = json!(true),
            5 => data["actions"] = json!(["terminate"]),
            6 => {
                data.as_object_mut().expect("metadata").remove("locked");
            }
            _ => {
                pod.status = Some("EXITED".into());
                data["runtime"] = json!({"uptime":1});
            }
        }
        pod.stop = serde_json::from_value(data).expect("raw metadata");
        fixture.transport.0.lock().expect("state").pods = vec![pod];
        assert_eq!(
            fixture.provider().stop_worker(&fixture.worker),
            Err(RunPodError::StopStateUnverified)
        );
        fixture.assert_calls(1, 0);
    }
}

#[test]
fn exact_ownership_is_checked_on_both_sides_of_stop() {
    for after_stop in [false, true] {
        for changed in 0..7 {
            let fixture = Fixture::new();
            let before = fixture.pod();
            let mut invalid = before.clone();
            match changed {
                0 => invalid.id = "another-pod".into(),
                1 => invalid.name = "foreign-name".into(),
                2 => invalid.image = format!("example/foreign@sha256:{}", "e".repeat(64)),
                3 => {
                    invalid
                        .env
                        .insert(WORKFLOW_ENV.into(), CloudWorkflowId::new().to_string());
                }
                4 => {
                    invalid.env.insert(JOB_ENV.into(), CloudJobId::new().to_string());
                }
                5 => {
                    invalid.env.insert(SSH_PUBLIC_KEY_ENV.into(), ed25519_key(9));
                }
                _ => {
                    invalid
                        .env
                        .insert(TERMINATE_ENV.into(), termination_deadline(900).expect("deadline"));
                }
            }
            fixture.transport.0.lock().expect("state").scripted_gets = if after_stop {
                vec![Ok(Some(before)), Ok(Some(invalid))]
            } else {
                vec![Ok(Some(invalid))]
            };
            assert_eq!(
                fixture.provider().stop_worker(&fixture.worker),
                Err(RunPodError::ResourceIdentityMismatch)
            );
            fixture.assert_calls(usize::from(after_stop) + 1, usize::from(after_stop));
        }
    }
}

#[test]
fn lost_stop_response_is_accepted_only_after_verified_retained_exit() {
    let fixture = Fixture::new();
    fixture.transport.0.lock().expect("state").stop_error = Some(stopping_failure());
    assert_eq!(
        fixture.provider().stop_worker(&fixture.worker),
        Ok(InteractiveWorkerStop::Stopped)
    );
    fixture.assert_calls(2, 1);
    for failed_command in [false, true] {
        let fixture = Fixture::new();
        let before = fixture.pod();
        {
            let mut state = fixture.transport.0.lock().expect("state");
            state.scripted_gets = vec![Ok(Some(before.clone())), Ok(Some(before))];
            state.stop_error = failed_command.then(stopping_failure);
        }
        let expected = if failed_command {
            stopping_failure()
        } else {
            RunPodError::StopVerificationFailed
        };
        assert_eq!(fixture.provider().stop_worker(&fixture.worker), Err(expected));
        fixture.assert_calls(2, 1);
    }
}

#[test]
fn absence_and_post_stop_retention_changes_are_never_stopped_acknowledgements() {
    let absent = Fixture::new();
    absent.transport.0.lock().expect("state").pods.clear();
    assert_eq!(
        absent.provider().stop_worker(&absent.worker),
        Ok(InteractiveWorkerStop::AlreadyAbsent)
    );
    absent.assert_calls(1, 0);
    for changed in 0..5 {
        let fixture = Fixture::new();
        let before = fixture.pod();
        let mut after = before.clone();
        after.status = Some("EXITED".into());
        let (observation, expected) = match changed {
            0 => (Ok(None), RunPodError::StopResourceLost),
            1 => {
                after.status = Some("TERMINATED".into());
                (Ok(Some(after)), RunPodError::StopStateUnverified)
            }
            2 => {
                let mut data = metadata();
                data["mounts"]["persistent"]["size"] = json!(21);
                after.stop = serde_json::from_value(data).expect("changed mount");
                (Ok(Some(after)), RunPodError::StopRetentionUnverified)
            }
            3 => {
                after.stop = StopMetadata::default();
                (Ok(Some(after)), RunPodError::StopRetentionUnverified)
            }
            _ => (Err(stopping_failure()), stopping_failure()),
        };
        fixture.transport.0.lock().expect("state").scripted_gets = vec![Ok(Some(before)), observation];
        assert_eq!(fixture.provider().stop_worker(&fixture.worker), Err(expected));
        fixture.assert_calls(2, 1);
    }
}

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn stored_fixture(store: &CloudWorkflowStore) -> (StoredRemoteAllocation, Fixture) {
    let state: RemoteWorkspaceState = serde_json::from_value(json!({
        "version":1, "spec":{"workspace_local_id":"workspace", "working_directory":".", "generation":0,
            "target":persistent_request().target,
            "repository":{"repository":"example/project", "commit":"b".repeat(40)}, "panels":[]}
    }))
    .expect("state");
    let dormant = store.create_remote_workspace(OWNER, &state).expect("dormant");
    let allocated = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocate");
    let reserved = store
        .reserve_remote_worker_request(&allocated, &ed25519_key(41))
        .expect("reserve");
    let fixture = Fixture::from_request(reserved.worker_request().expect("request"));
    let mut saved = reserved.workspace().state().clone();
    let runtime = saved.runtime.as_mut().expect("runtime");
    runtime.worker = Some(fixture.worker.clone());
    runtime.phase = RemoteRuntimePhase::Reconciling;
    store
        .replace_remote_workspace(reserved.workspace(), &saved)
        .expect("saved worker");
    (
        store
            .load_remote_allocation(OWNER, "workspace")
            .expect("read")
            .expect("allocation"),
        fixture,
    )
}

#[test]
fn real_store_intent_precedes_stop_and_uncertain_result_survives_controller_reopen() {
    let directory = tempfile::tempdir().expect("private fixture");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let (before, fixture) = stored_fixture(&store);
    let hook_store = store.clone();
    let expected_worker = fixture.worker.clone();
    let mut before_pod = fixture.pod();
    before_pod.status = Some("STARTING".into());
    {
        let mut state = fixture.transport.0.lock().expect("state");
        state.scripted_gets = vec![Ok(Some(before_pod.clone())), Ok(Some(before_pod))];
        state.on_stop = Some(Box::new(move || {
            let saved = hook_store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. }));
            assert_eq!(runtime.worker.as_ref(), Some(&expected_worker));
        }));
    }
    assert_eq!(
        stop_remote_workspace(&store, &fixture.provider(), before.workspace()),
        Err(RemoteWorkspaceStopError::ProviderUnavailable)
    );
    let pending = store
        .load_remote_allocation(OWNER, "workspace")
        .expect("read")
        .expect("pending");
    let requested = pending
        .workspace()
        .state()
        .runtime
        .as_ref()
        .expect("runtime")
        .phase
        .stop_requested_at_millis()
        .expect("intent");
    let mut expected = before.workspace().state().clone();
    expected.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Stopping {
        requested_at_millis: requested,
    };
    assert_eq!(pending.workspace().state(), &expected);
    assert_eq!(pending.workflow(), before.workflow());
    assert_eq!(pending.workspace().revision(), before.workspace().revision() + 1);
    let path = store.path().to_path_buf();
    fixture.transport.0.lock().expect("state").on_stop = None;
    drop(store);
    let reopened = CloudWorkflowStore::open_path(path).expect("reopened control store");
    let stopped = stop_remote_workspace(&reopened, &fixture.provider(), pending.workspace()).expect("verified retry");
    let phase = stopped.workspace().state().runtime.as_ref().expect("runtime").phase;
    assert!(
        matches!(phase, RemoteRuntimePhase::Stopped { requested_at_millis, .. } if requested_at_millis == requested)
    );
    expected.runtime.as_mut().expect("runtime").phase = phase;
    assert_eq!(stopped.workspace().state(), &expected);
    assert_eq!(stopped.workflow(), before.workflow());
    fixture.assert_calls(3, 1);
}
