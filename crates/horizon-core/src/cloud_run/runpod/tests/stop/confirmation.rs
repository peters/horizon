use super::*;
use crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint;
use crate::cloud_run::interactive_worker_stop::{
    InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation as Observation, InteractiveWorkerStopObserver,
};

fn ssh() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: "worker.example".into(),
        port: 2222,
        username: "root".into(),
        host_key: ed25519_key(73),
    }
}

fn observe(fixture: &Fixture) -> Result<Observation, RunPodError> {
    fixture
        .provider()
        .observe_worker_stop(InteractiveWorkerStopExpectation {
            worker: &fixture.worker,
            ssh: &ssh(),
            network_volume: None,
        })
}

#[test]
fn observation_reads_once_for_pending_retained_stopped_absence_and_failure_without_stop() {
    for (status, expected) in [
        ("RUNNING", Observation::Pending),
        ("STARTING", Observation::Pending),
        ("PROVISIONING", Observation::Pending),
        ("EXITED", Observation::RetainedStopped),
    ] {
        let fixture = Fixture::new();
        fixture.transport.0.lock().expect("state").pods[0].status = Some(status.into());
        assert_eq!(observe(&fixture), Ok(expected));
        fixture.assert_calls(1, 0);
    }
    let fixture = Fixture::new();
    fixture.transport.0.lock().expect("state").pods.clear();
    assert_eq!(observe(&fixture), Ok(Observation::Absent));
    fixture.assert_calls(1, 0);
    let fixture = Fixture::new();
    fixture.transport.0.lock().expect("state").scripted_gets = vec![Err(stopping_failure())];
    assert_eq!(observe(&fixture), Err(stopping_failure()));
    fixture.assert_calls(1, 0);
}

#[test]
fn malformed_pin_profile_worker_and_selection_refuse_before_any_read() {
    for changed in 0..6 {
        let fixture = Fixture::new();
        let mut worker = fixture.worker.clone();
        let mut pin = ssh();
        let selection = RunPodNetworkVolumeExpectation {
            volume_id: "other".into(),
            data_center_id: "other".into(),
            minimum_size_gb: 10,
        };
        match changed {
            0 => worker.identity.resource_id = "../foreign".into(),
            1 => worker.ssh_public_key.clear(),
            2 => worker.target.profile = "changed".into(),
            3 => pin.host_key.clear(),
            4 => pin.port = 0,
            _ => {}
        }
        assert!(
            fixture
                .provider()
                .observe_worker_stop(InteractiveWorkerStopExpectation {
                    worker: &worker,
                    ssh: &pin,
                    network_volume: (changed == 5).then_some(&selection),
                })
                .is_err()
        );
        fixture.assert_calls(0, 0);
    }
}

#[test]
fn terminal_or_malformed_metadata_cannot_be_promoted_to_retained_success() {
    for changed in 0..10 {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        pod.status = Some("EXITED".into());
        let mut data = metadata();
        match changed {
            0 => pod.status = Some("TERMINATED".into()),
            1 => pod.status = Some("UNKNOWN".into()),
            2 => data["runtime"] = json!({"uptime":1}),
            3 => data["mounts"]["persistent"]["path"] = json!("/other"),
            4 => data["mounts"]["persistent"]["size"] = json!(19),
            5 => data["mounts"] = json!({}),
            6 => data["cloud"] = json!("COMMUNITY"),
            7 => data["cluster"] = json!({"id":"foreign"}),
            8 => {
                pod.status = Some("RUNNING".into());
                data["locked"] = json!(true);
            }
            _ => {
                pod.status = Some("RUNNING".into());
                data["actions"] = json!(["terminate"]);
            }
        }
        pod.stop = serde_json::from_value(data).expect("metadata");
        fixture.transport.0.lock().expect("state").pods = vec![pod];
        assert!(observe(&fixture).is_err());
        fixture.assert_calls(1, 0);
    }
}

#[test]
fn observation_refuses_exact_identity_drift_without_mutation_or_host_key_lookup() {
    for changed in 0..6 {
        let fixture = Fixture::new();
        let mut pod = fixture.pod();
        match changed {
            0 => pod.id = "foreign".into(),
            1 => pod.name = "foreign".into(),
            2 => pod.image = format!("example/foreign@sha256:{}", "e".repeat(64)),
            3 => {
                pod.env.insert(WORKFLOW_ENV.into(), CloudWorkflowId::new().to_string());
            }
            4 => {
                pod.env.insert(JOB_ENV.into(), CloudJobId::new().to_string());
            }
            _ => {
                pod.env.insert(SSH_PUBLIC_KEY_ENV.into(), ed25519_key(9));
            }
        }
        fixture.transport.0.lock().expect("state").scripted_gets = vec![Ok(Some(pod))];
        assert_eq!(observe(&fixture), Err(RunPodError::ResourceIdentityMismatch));
        fixture.assert_calls(1, 0);
    }
}

#[test]
fn actual_provider_confirmation_completes_delayed_stop_without_resending() {
    let directory = tempfile::tempdir().expect("store");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let (allocated, fixture) = stored_fixture(&store);
    let mut saved = allocated.workspace().state().clone();
    saved.runtime.as_mut().expect("runtime").ssh = Some(ssh());
    store
        .replace_remote_workspace(allocated.workspace(), &saved)
        .expect("saved pin");
    let current = store
        .load_remote_allocation(OWNER, "workspace")
        .expect("read")
        .expect("allocation");
    let pending = store
        .record_remote_stop_phase(&current, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
        .expect("saved intent");
    let first = crate::remote_workspace::stop::confirm_remote_workspace_stop(&store, &fixture.provider(), &pending)
        .expect("pending read");
    assert_eq!(first.observation, Observation::Pending);
    assert_eq!(first.allocation, pending);
    fixture.assert_calls(1, 0);
    fixture.transport.0.lock().expect("state").pods[0].status = Some("EXITED".into());
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("fresh controller");
    let confirmed =
        crate::remote_workspace::stop::confirm_remote_workspace_stop(&reopened, &fixture.provider(), &pending)
            .expect("retained stop");
    assert_eq!(confirmed.observation, Observation::RetainedStopped);
    assert!(matches!(
        confirmed
            .allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .phase,
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            ..
        }
    ));
    fixture.assert_calls(2, 0);
}
