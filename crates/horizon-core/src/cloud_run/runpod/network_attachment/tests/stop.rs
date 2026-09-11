use super::*;
use crate::{
    cloud_run::{CloudWorkflowStore, interactive_worker_stop::InteractiveWorkerStop},
    remote_workspace::stop::{RemoteWorkspaceStopError, stop_remote_workspace},
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
};

fn metadata() -> Value {
    json!({"mounts":{"network":[{"volumeId":"volume_exact","path":"/workspace"}]},
        "cloud":"SECURE", "dataCenterId":"EUR-NO-1", "cluster":null,
        "locked":false, "actions":["stop","terminate"], "runtime":null})
}

fn prepare(fixture: &Fixture) {
    let mut state = fixture.transport.0.lock().expect("state");
    state.template.stop = serde_json::from_value(metadata()).expect("metadata");
    state.pods = vec![state.template.clone()];
}

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    prepare(&fixture);
    fixture
}

fn pod(fixture: &Fixture) -> ApiPod {
    fixture.transport.0.lock().expect("state").pods[0].clone()
}

fn assert_calls(fixture: &Fixture, calls: &[&str]) {
    let state = fixture.transport.0.lock().expect("state");
    assert_eq!(state.calls, calls);
    assert!(state.creates.is_empty() && state.deleted.is_empty());
    assert_eq!(fixture.claims.load(Ordering::SeqCst), 0);
}

fn failed_stop() -> RunPodError {
    RunPodError::RequestFailed { operation: "pod Stop" }
}

#[test]
fn one_stop_verifies_both_sides_and_already_stopped_needs_no_mutation() {
    for lost_reply in [false, true] {
        let fixture = fixture();
        let before = pod(&fixture);
        fixture.transport.0.lock().expect("state").stop_error = lost_reply.then(failed_stop);
        // The selected network volume replaces the ordinary profile volume.
        let mut configuration = profile();
        configuration.volume_gib = 0;
        let provider = RunPodInteractiveWorkerProvider::new_with_network_volume(
            fixture.client(),
            configuration,
            fixture.trust(),
            &fixture.request,
            &fixture.selection,
        )
        .expect("provider");
        assert_eq!(
            provider.stop_worker(&fixture.worker()),
            Ok(InteractiveWorkerStop::Stopped)
        );
        assert_calls(&fixture, &["get", "volume", "stop", "get", "volume"]);
        let after = pod(&fixture);
        assert_eq!(after.env, before.env);
        assert_eq!(after.id, before.id);
        assert_eq!(after.status.as_deref(), Some("EXITED"));
        assert_eq!(
            provider.stop_worker(&fixture.worker()),
            Ok(InteractiveWorkerStop::Stopped)
        );
        assert_calls(&fixture, &["get", "volume", "stop", "get", "volume", "get", "volume"]);
    }
}

#[test]
fn still_running_is_not_retained_stopped_even_after_a_successful_response() {
    for lost_reply in [false, true] {
        let fixture = fixture();
        let before = pod(&fixture);
        let mut state = fixture.transport.0.lock().expect("state");
        state.scripted_gets = [Ok(Some(before.clone())), Ok(Some(before))].into();
        state.stop_error = lost_reply.then(failed_stop);
        drop(state);
        let expected = if lost_reply {
            failed_stop()
        } else {
            RunPodError::StopVerificationFailed
        };
        assert_eq!(fixture.provider().stop_worker(&fixture.worker()), Err(expected));
        assert_calls(&fixture, &["get", "volume", "stop", "get", "volume"]);
    }
}

#[test]
fn exact_volume_metadata_must_match_before_and_after_stop() {
    for after_stop in [false, true] {
        for changed in 0..6 {
            let fixture = fixture();
            let mut state = fixture.transport.0.lock().expect("state");
            let valid = state.volume.clone();
            let mut changed_volume = valid.clone().expect("volume");
            match changed {
                0 => changed_volume["id"] = json!("foreign"),
                1 => changed_volume["dataCenter"] = json!("foreign"),
                2 => changed_volume["type"] = json!("STANDARD"),
                3 => changed_volume["size"] = json!(9),
                _ => {}
            }
            let invalid = match changed {
                4 => Ok(None),
                5 => Err(failed_stop()),
                _ => Ok(Some(changed_volume)),
            };
            if after_stop {
                state.scripted_volumes.push_back(Ok(valid));
            }
            state.scripted_volumes.push_back(invalid);
            drop(state);
            let expected = if changed == 5 {
                failed_stop()
            } else {
                RunPodError::ResourceIdentityMismatch
            };
            assert_eq!(fixture.provider().stop_worker(&fixture.worker()), Err(expected));
            assert_calls(
                &fixture,
                if after_stop {
                    &["get", "volume", "stop", "get", "volume"]
                } else {
                    &["get", "volume"]
                },
            );
        }
    }
}

#[test]
fn exact_attachment_and_worker_identity_are_verified_on_both_sides() {
    for after_stop in [false, true] {
        for changed in 0..11 {
            let fixture = fixture();
            let before = pod(&fixture);
            let mut invalid = before.clone();
            let mut data = metadata();
            match changed {
                0 => data["mounts"]["network"][0]["volumeId"] = json!("foreign"),
                1 => data["mounts"]["network"][0]["path"] = json!("/other"),
                2 => data["dataCenterId"] = json!("foreign"),
                3 => data["mounts"]["network"] = json!([]),
                4 => data["mounts"]["persistent"] = json!({"path":"/workspace","size":20}),
                5 => invalid.id = "other_pod".into(),
                6 => invalid.name = "foreign".into(),
                7 => invalid.image = format!("example/other@sha256:{}", "a".repeat(64)),
                8 => {
                    invalid
                        .env
                        .insert("HORIZON_WORKFLOW_ID".into(), CloudWorkflowId::new().to_string());
                }
                9 => {
                    invalid
                        .env
                        .insert("HORIZON_JOB_ID".into(), CloudJobId::new().to_string());
                }
                _ => {
                    invalid.env.insert("HORIZON_SSH_PUBLIC_KEY".into(), ed25519_key(90));
                }
            }
            invalid.stop = serde_json::from_value(data).expect("metadata");
            let mut state = fixture.transport.0.lock().expect("state");
            if after_stop {
                state.scripted_gets.push_back(Ok(Some(before)));
            }
            state.scripted_gets.push_back(Ok(Some(invalid)));
            drop(state);
            assert_eq!(
                fixture.provider().stop_worker(&fixture.worker()),
                Err(RunPodError::ResourceIdentityMismatch)
            );
            let mut expected = if after_stop {
                vec!["get", "volume", "stop", "get"]
            } else {
                vec!["get"]
            };
            if changed < 5 {
                expected.push("volume");
            }
            assert_calls(&fixture, &expected);
        }
    }
}

#[test]
fn selected_stop_keeps_positive_state_and_capability_requirements() {
    for after_stop in [false, true] {
        for changed in 0..9 {
            let fixture = fixture();
            let before = pod(&fixture);
            let mut invalid = before.clone();
            let mut data = metadata();
            match changed {
                0 => data["locked"] = json!(true),
                1 => data["actions"] = json!(["terminate"]),
                2 => data["cloud"] = json!("COMMUNITY"),
                3 => data["cluster"] = json!({"id":"foreign"}),
                4 => invalid.status = Some("ERROR".into()),
                5 => invalid.status = None,
                6 => invalid.status = Some("TERMINATED".into()),
                7 => invalid.status = Some("unknown".into()),
                _ => {
                    invalid.status = Some("EXITED".into());
                    data["runtime"] = json!({"uptime":1});
                }
            }
            invalid.stop = serde_json::from_value(data).expect("metadata");
            let mut state = fixture.transport.0.lock().expect("state");
            if after_stop {
                state.scripted_gets.push_back(Ok(Some(before)));
            }
            state.scripted_gets.push_back(Ok(Some(invalid)));
            drop(state);
            let retention = changed == 2 || changed == 3;
            let error = if retention {
                RunPodError::StopRetentionUnverified
            } else {
                RunPodError::StopStateUnverified
            };
            assert_eq!(fixture.provider().stop_worker(&fixture.worker()), Err(error));
            let mut expected = if after_stop {
                vec!["get", "volume", "stop", "get"]
            } else {
                vec!["get"]
            };
            if !retention {
                expected.push("volume");
            }
            assert_calls(&fixture, &expected);
        }
    }
}

#[test]
fn absence_and_unknown_reads_never_acknowledge_a_retained_stop() {
    let absent = fixture();
    absent.transport.0.lock().expect("state").pods.clear();
    assert_eq!(
        absent.provider().stop_worker(&absent.worker()),
        Ok(InteractiveWorkerStop::AlreadyAbsent)
    );
    assert_calls(&absent, &["get"]);
    for after_stop in [false, true] {
        let fixture = fixture();
        let before = pod(&fixture);
        let mut state = fixture.transport.0.lock().expect("state");
        if after_stop {
            state.scripted_gets.push_back(Ok(Some(before)));
        }
        state.scripted_gets.push_back(Err(failed_stop()));
        drop(state);
        assert_eq!(fixture.provider().stop_worker(&fixture.worker()), Err(failed_stop()));
        assert_calls(
            &fixture,
            if after_stop {
                &["get", "volume", "stop", "get"]
            } else {
                &["get"]
            },
        );
    }
    let lost = fixture();
    let before = pod(&lost);
    lost.transport.0.lock().expect("state").scripted_gets = [Ok(Some(before)), Ok(None)].into();
    assert_eq!(
        lost.provider().stop_worker(&lost.worker()),
        Err(RunPodError::StopResourceLost)
    );
    assert_calls(&lost, &["get", "volume", "stop", "get"]);
}

#[test]
fn provisioning_stop_works_but_ordinary_provider_cannot_adopt_network_retention() {
    for status in ["PROVISIONING", "STARTING"] {
        let fixture = fixture();
        fixture.transport.0.lock().expect("state").pods[0].status = Some(status.into());
        let ordinary = RunPodInteractiveWorkerProvider::new(fixture.client(), profile(), fixture.trust());
        assert_eq!(
            ordinary.stop_worker(&fixture.worker()),
            Err(RunPodError::StopRetentionUnverified)
        );
        assert_calls(&fixture, &["get"]);
        assert_eq!(
            fixture.provider().stop_worker(&fixture.worker()),
            Ok(InteractiveWorkerStop::Stopped)
        );
        assert_calls(&fixture, &["get", "get", "volume", "stop", "get", "volume"]);
    }
}

#[test]
fn durable_intent_selection_and_identity_survive_unverified_stop() {
    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    let directory = tempfile::tempdir().expect("private fixture");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let fixture = Fixture::new();
    let workspace: RemoteWorkspaceState = serde_json::from_value(json!({
        "version":1,"spec":{"workspace_local_id":"workspace","working_directory":".","generation":0,
            "target":fixture.request.target,"repository":{"repository":"example/project","commit":"b".repeat(40)},"panels":[]}
    })).expect("workspace");
    let dormant = store.create_remote_workspace(OWNER, &workspace).expect("create");
    let allocated = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocate");
    store
        .record_remote_network_volume_selection(&allocated, &fixture.selection)
        .expect("selection");
    let reserved = store
        .reserve_remote_worker_request(&allocated, &fixture.request.ssh_public_key)
        .expect("reserve");
    let fixture = Fixture::from_request(reserved.worker_request().expect("request"));
    prepare(&fixture);
    let mut saved = reserved.workspace().state().clone();
    let runtime = saved.runtime.as_mut().expect("runtime");
    runtime.worker = Some(fixture.worker());
    runtime.phase = RemoteRuntimePhase::Reconciling;
    let saved = store
        .replace_remote_workspace(reserved.workspace(), &saved)
        .expect("worker");
    let hook_store = store.clone();
    let worker = fixture.worker();
    let before = pod(&fixture);
    {
        let mut state = fixture.transport.0.lock().expect("state");
        state.scripted_gets = [Ok(Some(before.clone())), Ok(Some(before))].into();
        state.stop_error = Some(failed_stop());
        state.on_stop = Some(Box::new(move || {
            let pending = hook_store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            let runtime = pending.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. }));
            assert_eq!(runtime.worker.as_ref(), Some(&worker));
            assert!(
                hook_store
                    .load_remote_network_volume_selection(&pending)
                    .expect("selection")
                    .is_some()
            );
        }));
    }
    assert_eq!(
        stop_remote_workspace(&store, &fixture.provider(), &saved),
        Err(RemoteWorkspaceStopError::ProviderUnavailable)
    );
    let path = store.path().to_path_buf();
    fixture.transport.0.lock().expect("state").on_stop = None;
    drop(store);
    let reopened = CloudWorkflowStore::open_path(path).expect("reopen");
    let pending = reopened
        .load_remote_allocation(OWNER, "workspace")
        .expect("read")
        .expect("pending");
    let phase = pending.workspace().state().runtime.as_ref().expect("runtime").phase;
    assert!(matches!(phase, RemoteRuntimePhase::Stopping { .. }));
    let mut expected = saved.state().clone();
    expected.runtime.as_mut().expect("runtime").phase = phase;
    assert_eq!(pending.workspace().state(), &expected);
    assert_eq!(
        reopened
            .load_remote_network_volume_selection(&pending)
            .expect("selection"),
        Some(fixture.selection.clone())
    );
    let stopped = stop_remote_workspace(&reopened, &fixture.provider(), pending.workspace()).expect("observed stopped");
    assert!(matches!(
        stopped.workspace().state().runtime.as_ref().expect("runtime").phase,
        RemoteRuntimePhase::Stopped { .. }
    ));
    assert_eq!(
        stopped.workspace().state().runtime.as_ref().expect("runtime").worker,
        expected.runtime.expect("runtime").worker
    );
    assert_eq!(stopped.workflow(), pending.workflow());
    assert_calls(&fixture, &["get", "volume", "stop", "get", "volume", "get", "volume"]);
}
