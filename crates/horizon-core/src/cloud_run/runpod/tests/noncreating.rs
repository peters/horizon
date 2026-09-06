use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn read_only_provider(transport: FakeTransport) -> (RunPodInteractiveWorkerProvider, Arc<AtomicUsize>) {
    let claims = Arc::new(AtomicUsize::new(0));
    let calls = claims.clone();
    let client = RunPodClient {
        transport: Box::new(transport),
        creation_fence: Box::new(move |_, _, _: &WorkerTarget, _: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }),
    };
    (
        RunPodInteractiveWorkerProvider::new(client, profile(), FakeHostKeySource::new(Some(ed25519_key(73)))),
        claims,
    )
}

fn assert_read_only(transport: &FakeTransport, claims: &AtomicUsize) {
    let state = transport.0.lock().expect("state");
    assert!(state.create_requests.is_empty());
    assert!(state.deleted.is_empty());
    assert_eq!(claims.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_absence_never_consumes_an_available_creation_grant() {
    let request = persistence::persistent_request();
    let transport = FakeTransport::default();
    for _ in 0..3 {
        let (provider, claims) = read_only_provider(transport.clone());
        assert_eq!(provider.reconcile_worker(&request), Ok(None));
        assert_read_only(&transport, &claims);
    }
    assert_eq!(
        transport.0.lock().expect("state").list_calls,
        3 * (1 + http::PROPAGATION_BACKOFF_MS.len())
    );
}

#[test]
fn disappearance_during_discovery_returns_absence_not_cached_readiness() {
    let request = persistence::persistent_request();
    let pod = persistence::persistent_pod(&request);
    let transport = FakeTransport::default();
    transport.0.lock().expect("state").scripted_lists = vec![vec![pod.clone()]];
    let (provider, claims) = read_only_provider(transport.clone());
    assert_eq!(provider.reconcile_worker(&request), Ok(None));
    assert_eq!(transport.0.lock().expect("state").inspected, vec![pod.id]);
    assert_read_only(&transport, &claims);
}

#[test]
fn exact_refresh_rejects_errors_identity_drift_and_new_cost_without_cleanup() {
    let request = persistence::persistent_request();
    for drift in 0..5 {
        let original = persistence::persistent_pod(&request);
        let mut fresh = original.clone();
        match drift {
            0 => fresh.id = "redirected-resource".into(),
            1 => {
                fresh.env.insert(SSH_PUBLIC_KEY_ENV.into(), ed25519_key(99));
            }
            2 => fresh.image = "different:latest".into(),
            3 => fresh.cost = Some(750_001),
            _ => {}
        }
        let transport = FakeTransport::with_pods(vec![original.clone()]);
        transport.0.lock().expect("state").scripted_gets = vec![if drift == 4 {
            Err(RunPodError::RequestFailed {
                operation: "pod inspection",
            })
        } else {
            Ok(Some(fresh))
        }];
        let (provider, claims) = read_only_provider(transport.clone());
        let error = provider
            .reconcile_worker(&request)
            .expect_err("fresh read is authoritative");
        if drift == 3 {
            assert!(matches!(
                error,
                RunPodError::WorkerRecoveryCostRejected {
                    actual: Some(750_001),
                    ..
                }
            ));
        } else if drift == 4 {
            assert!(matches!(
                error,
                RunPodError::RequestFailed {
                    operation: "pod inspection"
                }
            ));
        } else {
            assert_eq!(error, RunPodError::ResourceIdentityMismatch);
        }
        assert_eq!(transport.0.lock().expect("state").inspected, vec![original.id]);
        assert_read_only(&transport, &claims);
    }
}

#[test]
fn lost_handle_recovery_retains_the_same_worker_and_never_restarts_stopped_tasks() {
    let request = persistence::persistent_request();
    let transport = FakeTransport::with_pods(vec![persistence::persistent_pod(&request)]);
    let (provider, claims) = read_only_provider(transport.clone());
    let original = provider.reconcile_worker(&request).expect("recover").expect("existing");
    assert!(original.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_read_only(&transport, &claims);
    drop(provider);
    for (state, expected) in [
        ("EXITED", InteractiveWorkerLifecycle::Stopped),
        ("ERROR", InteractiveWorkerLifecycle::Failed),
        ("unknown", InteractiveWorkerLifecycle::Unknown),
    ] {
        transport.0.lock().expect("state").pods[0].status = Some(state.into());
        let (provider, claims) = read_only_provider(transport.clone());
        let recovered = provider.reconcile_worker(&request).expect("recover").expect("retained");
        assert_eq!(recovered.worker, original.worker);
        assert_eq!(recovered.lifecycle, expected);
        assert!(!recovered.is_ready_for(&request, time::OffsetDateTime::now_utc()));
        assert_read_only(&transport, &claims);
    }
}

#[test]
fn invalid_or_ambiguous_resources_do_not_create_or_delete() {
    let request = persistence::persistent_request();
    let good = persistence::persistent_pod(&request);
    for drift in 0..6 {
        let mut pod = good.clone();
        match drift {
            0 => {
                pod.env.insert(JOB_ENV.into(), CloudJobId::new().to_string());
            }
            1 => {
                pod.env.insert(SSH_PUBLIC_KEY_ENV.into(), ed25519_key(99));
            }
            2 => {
                pod.env.insert(TERMINATE_ENV.into(), String::new());
            }
            3 => pod.image = "mutable:latest".into(),
            4 => pod.name = "different".into(),
            _ => {
                pod.env.remove(LIFETIME_ENV);
            }
        }
        let transport = FakeTransport::with_pods(vec![pod]);
        let (provider, claims) = read_only_provider(transport.clone());
        assert_eq!(
            provider.reconcile_worker(&request),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_read_only(&transport, &claims);
    }
    let mut duplicate = good.clone();
    duplicate.id = "other-exact-id".into();
    let transport = FakeTransport::with_pods(vec![good, duplicate]);
    let (provider, claims) = read_only_provider(transport.clone());
    assert!(matches!(
        provider.reconcile_worker(&request),
        Err(RunPodError::AmbiguousResource { count: 2, .. })
    ));
    assert_read_only(&transport, &claims);
}

#[test]
fn malformed_recovery_requests_fail_before_any_transport_or_claim() {
    let request = persistence::persistent_request();
    let transport = FakeTransport::default();
    let (provider, claims) = read_only_provider(transport.clone());
    for drift in 0..4 {
        let mut invalid = request.clone();
        match drift {
            0 => invalid.target.provider = CloudProvider::LocalDocker,
            1 => invalid.target.profile = "different".into(),
            2 => invalid.target.image = "mutable:latest".into(),
            _ => invalid.ssh_public_key = "invalid".into(),
        }
        assert_eq!(provider.reconcile_worker(&invalid), Err(RunPodError::InvalidTarget));
    }
    assert_eq!(transport.0.lock().expect("state").list_calls, 0);
    assert_read_only(&transport, &claims);
}

#[test]
fn expired_and_over_budget_recovery_never_performs_automatic_cleanup() {
    let timed = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
    let mut pod = running_pod(timed.workflow_id, timed.job_id, &timed.target);
    pod.env.insert(SSH_PUBLIC_KEY_ENV.into(), timed.ssh_public_key.clone());
    pod.env.insert(TERMINATE_ENV.into(), "2000-01-01T00:00:00Z".into());
    let transport = FakeTransport::with_pods(vec![pod]);
    let (provider, claims) = read_only_provider(transport.clone());
    let expired = provider
        .reconcile_worker(&timed)
        .expect("recover expired")
        .expect("retained");
    assert!(!expired.is_ready_for(&timed, time::OffsetDateTime::now_utc()));
    assert_read_only(&transport, &claims);

    for persistent in [false, true] {
        for cost in [None, Some(750_001)] {
            let request = if persistent {
                persistence::persistent_request()
            } else {
                timed.clone()
            };
            let mut pod = if persistent {
                persistence::persistent_pod(&request)
            } else {
                let mut pod = running_pod(request.workflow_id, request.job_id, &request.target);
                pod.env
                    .insert(SSH_PUBLIC_KEY_ENV.into(), request.ssh_public_key.clone());
                pod
            };
            pod.cost = cost;
            let transport = FakeTransport::with_pods(vec![pod]);
            let (provider, claims) = read_only_provider(transport.clone());
            let error = provider
                .reconcile_worker(&request)
                .expect_err("read-only cost rejection");
            assert!(error.to_string().contains("not deleted and may remain billable"));
            let RunPodError::WorkerRecoveryCostRejected {
                worker,
                actual,
                maximum,
            } = error
            else {
                panic!("cost error")
            };
            assert_eq!(worker.pod_id, "pod_123456");
            assert_eq!(actual, cost);
            assert_eq!(maximum, 750_000);
            assert_read_only(&transport, &claims);
        }
    }
}
