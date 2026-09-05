use super::*;

pub(super) fn persistent_request() -> InteractiveWorkerRequest {
    let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
    request.target.lifetime = WorkerLifetime::Persistent;
    request
}

pub(super) fn persistent_pod(request: &InteractiveWorkerRequest) -> ApiPod {
    let mut pod = running_pod(request.workflow_id, request.job_id, &target());
    pod.env.remove(TERMINATE_ENV);
    pod.env.insert(LIFETIME_ENV.into(), PERSISTENT_LIFETIME.into());
    pod.env
        .insert(SSH_PUBLIC_KEY_ENV.into(), request.ssh_public_key.clone());
    pod
}

pub(super) fn provider(transport: FakeTransport) -> RunPodInteractiveWorkerProvider {
    RunPodInteractiveWorkerProvider::new(
        RunPodClient::with_transport(transport),
        profile(),
        FakeHostKeySource::new(Some(ed25519_key(73))),
    )
}

#[test]
fn persistent_creation_survives_controller_replacement_and_explicit_cleanup_is_exact() {
    let request = persistent_request();
    let mut response = persistent_pod(&request);
    response.env.clear();
    let transport = FakeTransport::with_create_response(response);
    let original = provider(transport.clone());
    let InteractiveWorkerEnsure::Created(status) = original.ensure_worker(&request).expect("persistent creation")
    else {
        panic!("expected one creation");
    };
    assert_eq!(status.worker.lifetime, InteractiveWorkerLifetime::Persistent);
    assert!(status.is_ready_for(
        &request,
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(36_500)
    ));
    let encoded = serde_json::to_string(&status.worker).expect("persist worker");
    let restored = serde_json::from_str(&encoded).expect("restore worker");
    drop(original);
    let reopened = provider(transport.clone());
    assert_eq!(reopened.inspect_worker(&restored), Ok(Some(status.clone())));
    assert_eq!(
        reopened.ensure_worker(&request),
        Ok(InteractiveWorkerEnsure::Reused(status))
    );
    {
        let state = transport.0.lock().expect("state");
        assert_eq!(state.create_requests.len(), 1);
        assert!(state.deleted.is_empty());
        let payload = serde_json::to_value(&state.create_requests[0]).expect("creation payload");
        for field in ["terminateAfter", "stopAfter"] {
            assert!(payload.get(field).is_none());
        }
        let env = &state.create_requests[0].env;
        assert!(!env.iter().any(|entry| entry.key == TERMINATE_ENV));
        let markers: Vec<_> = env.iter().filter(|entry| entry.key == LIFETIME_ENV).collect();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].value, PERSISTENT_LIFETIME);
    }
    assert_eq!(reopened.delete_worker(&restored), Ok(InteractiveWorkerCleanup::Deleted));
    assert_eq!(
        reopened.delete_worker(&restored),
        Ok(InteractiveWorkerCleanup::AlreadyAbsent)
    );
    assert_eq!(transport.0.lock().expect("state").deleted, ["pod_123456"]);
}

#[test]
fn stopped_unknown_and_missing_persistent_workers_are_not_restarted_or_replaced() {
    let request = persistent_request();
    let transport = FakeTransport::with_pods(vec![persistent_pod(&request)]);
    let provider = provider(transport.clone());
    let worker = provider
        .ensure_worker(&request)
        .expect("existing worker")
        .into_status()
        .worker;
    for (state, expected) in [
        ("EXITED", InteractiveWorkerLifecycle::Stopped),
        ("UNRECOGNIZED", InteractiveWorkerLifecycle::Unknown),
    ] {
        transport.0.lock().expect("state").pods[0].status = Some(state.into());
        let observed = provider.inspect_worker(&worker).expect("inspect").expect("retained");
        assert_eq!(observed.worker, worker);
        assert_eq!(observed.lifecycle, expected);
        assert!(observed.ssh.is_none());
        assert_eq!(
            provider.ensure_worker(&request),
            Ok(InteractiveWorkerEnsure::Reused(observed))
        );
    }
    transport.0.lock().expect("state").pods.clear();
    assert_eq!(provider.inspect_worker(&worker), Ok(None));
    let state = transport.0.lock().expect("state");
    assert!(state.create_requests.is_empty());
    assert!(state.deleted.is_empty());
}

#[test]
fn persistent_metadata_and_client_identity_drift_never_authorize_reuse_or_deletion() {
    for drift in 0..9 {
        let request = persistent_request();
        let transport = FakeTransport::with_pods(vec![persistent_pod(&request)]);
        let provider = provider(transport.clone());
        let worker = provider.ensure_worker(&request).expect("original").into_status().worker;
        {
            let mut state = transport.0.lock().expect("state");
            let pod = &mut state.pods[0];
            match drift {
                0 => {
                    pod.env.remove(LIFETIME_ENV);
                }
                1 => {
                    pod.env.insert(LIFETIME_ENV.into(), String::new());
                }
                2 => {
                    pod.env.insert(LIFETIME_ENV.into(), "unknown".into());
                }
                3 => {
                    pod.env.insert(TERMINATE_ENV.into(), String::new());
                }
                4 => {
                    pod.env.insert(TERMINATE_ENV.into(), "2000-01-01T00:00:00Z".into());
                }
                5 => {
                    pod.env.insert(SSH_PUBLIC_KEY_ENV.into(), ed25519_key(99));
                }
                6 => {
                    pod.env.insert(JOB_ENV.into(), CloudJobId::new().to_string());
                }
                7 => {
                    pod.env.insert(WORKFLOW_ENV.into(), CloudWorkflowId::new().to_string());
                }
                _ => {
                    pod.image = format!("registry.example/other@sha256:{}", "b".repeat(64));
                }
            }
        }
        assert_eq!(
            provider.ensure_worker(&request),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_eq!(
            provider.inspect_worker(&worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_eq!(
            provider.delete_worker(&worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        let state = transport.0.lock().expect("state");
        assert!(state.create_requests.is_empty());
        assert!(state.deleted.is_empty());
        assert_eq!(state.pods.len(), 1);
    }
}

#[test]
fn cross_policy_recovery_and_handles_cannot_adopt_or_delete_the_other_lifetime() {
    for persistent_first in [false, true] {
        let mut request = persistent_request();
        let pod = if persistent_first {
            persistent_pod(&request)
        } else {
            request.target.lifetime = target().lifetime;
            let mut pod = running_pod(request.workflow_id, request.job_id, &request.target);
            pod.env
                .insert(SSH_PUBLIC_KEY_ENV.into(), request.ssh_public_key.clone());
            pod
        };
        let transport = FakeTransport::with_pods(vec![pod]);
        let provider = provider(transport.clone());
        let mut worker = provider.ensure_worker(&request).expect("original").into_status().worker;
        request.target.lifetime = if persistent_first {
            target().lifetime
        } else {
            WorkerLifetime::Persistent
        };
        assert_eq!(
            provider.ensure_worker(&request),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        worker.target = request.target.clone();
        worker.lifetime = if persistent_first {
            InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                terminate_after: termination_deadline(3600).expect("deadline"),
            })
        } else {
            InteractiveWorkerLifetime::Persistent
        };
        assert!(worker.is_valid_for(CloudProvider::RunPod));
        assert_eq!(
            provider.inspect_worker(&worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_eq!(
            provider.delete_worker(&worker),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        let state = transport.0.lock().expect("state");
        assert!(state.create_requests.is_empty());
        assert!(state.deleted.is_empty());
    }
}

#[test]
fn new_and_recovered_persistent_cost_rejections_retain_the_worker_and_report_billing() {
    for create in [false, true] {
        for cost in [None, Some(0), Some(750_001)] {
            let request = persistent_request();
            let mut pod = persistent_pod(&request);
            pod.cost = cost;
            let transport = if create {
                FakeTransport::with_create_response(pod)
            } else {
                FakeTransport::with_pods(vec![pod])
            };
            let error = provider(transport.clone())
                .ensure_worker(&request)
                .expect_err("cost rejection");
            assert!(error.to_string().contains("not deleted and may remain billable"));
            let RunPodError::PersistentWorkerCostRejected {
                worker,
                actual,
                maximum,
            } = error
            else {
                panic!("expected retained cost error");
            };
            assert_eq!(worker.pod_id, "pod_123456");
            assert_eq!(worker.lifetime, InteractiveWorkerLifetime::Persistent);
            assert_eq!(actual, cost.filter(|value| *value > 0));
            assert_eq!(maximum, 750_000);
            let state = transport.0.lock().expect("state");
            assert_eq!(state.create_requests.len(), usize::from(create));
            assert!(state.deleted.is_empty());
            assert_eq!(state.pods.len(), 1);
        }
    }
}

#[test]
fn conflicting_new_persistent_metadata_returns_the_exact_identity_without_cleanup() {
    for drift in 0..3 {
        let request = persistent_request();
        let mut pod = persistent_pod(&request);
        match drift {
            0 => {
                pod.env.insert(TERMINATE_ENV.into(), "2000-01-01T00:00:00Z".into());
            }
            1 => {
                pod.env.insert(TERMINATE_ENV.into(), String::new());
            }
            _ => {
                pod.image = format!("registry.example/other@sha256:{}", "b".repeat(64));
            }
        }
        let transport = FakeTransport::with_create_response(pod);
        assert_eq!(
            provider(transport.clone()).ensure_worker(&request),
            Err(RunPodError::PersistentCreationReconciliationRequired {
                name: resource_name(request.workflow_id, request.job_id),
                pod_id: "pod_123456".into(),
            })
        );
        let state = transport.0.lock().expect("state");
        assert_eq!(state.create_requests.len(), 1);
        assert!(state.deleted.is_empty());
        assert_eq!(state.pods.len(), 1);
    }
}
