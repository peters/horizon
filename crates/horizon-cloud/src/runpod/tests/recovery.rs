use super::*;
use crate::runpod::recovery::Outcome;

#[test]
fn delayed_visibility_survives_restart_and_never_posts() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (200, "[]".into()),
        (200, json!([worker(&spec)]).to_string()),
    ]);
    let mut state = CreateState::Requested;
    for expected in [
        Outcome::Unresolved,
        Outcome::Unresolved,
        Outcome::Found {
            worker_id: "worker1".into(),
        },
    ] {
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let report = provider
            .reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        assert_eq!(report.outcome, expected);
    }
    assert_eq!(
        state,
        CreateState::Bound {
            worker_id: "worker1".into()
        }
    );
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

#[test]
fn maximum_length_verified_operator_hint_recovers_an_unlisted_worker() {
    let spec = spec();
    let worker_id = "w".repeat(100);
    let mut found = worker(&spec);
    found["id"] = json!(worker_id);
    let (provider, requests, task) = server(vec![(200, "[]".into()), (200, found.to_string())]);
    let mut state = CreateState::Requested;
    let mut saved = Vec::new();
    let report = provider
        .reconcile(&spec, &mut state, Some(&worker_id), &Cancellation::default(), |next| {
            saved.push(next.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Found {
            worker_id: worker_id.clone()
        }
    );
    assert_eq!(saved, vec![state]);
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("publicIp"));
    assert!(!encoded.contains("env"));
    task.join().unwrap();
    assert!(requests.lock().unwrap()[1].starts_with(&format!("GET /pods/{worker_id} ")));
}

#[test]
fn missing_hint_and_empty_account_do_not_prove_rejection_or_deletion() {
    let (provider, requests, task) = server(vec![(200, "[]".into()), (404, "{}".into())]);
    let mut state = CreateState::Requested;
    let report = provider
        .reconcile(&spec(), &mut state, Some("worker1"), &Cancellation::default(), |_| {
            panic!("An unknown outcome cannot change the persisted fence")
        })
        .unwrap();
    assert_eq!(report.outcome, Outcome::Unresolved);
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn unrelated_hint_is_not_an_operator_override() {
    let mut unrelated = worker(&spec());
    unrelated["name"] = json!("another-cloud");
    let (provider, requests, task) = server(vec![(200, "[]".into()), (200, unrelated.to_string())]);
    let mut state = CreateState::Requested;
    assert!(matches!(
        provider.reconcile(&spec(), &mut state, Some("worker1"), &Cancellation::default(), |_| Ok(
            ()
        )),
        Err(CloudError::IdentityMismatch)
    ));
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

#[test]
fn operator_hint_cannot_select_away_a_second_match() {
    let spec = spec();
    let mut other = worker(&spec);
    other["id"] = json!("worker2");
    let (provider, _, task) = server(vec![(200, json!([worker(&spec), other]).to_string())]);
    let mut state = CreateState::Requested;
    let report = provider
        .reconcile(&spec, &mut state, Some("worker1"), &Cancellation::default(), |_| {
            panic!("Conflicting evidence cannot select a worker")
        })
        .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Conflicting {
            worker_ids: vec!["worker1".into(), "worker2".into()]
        }
    );
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
}

#[test]
fn mismatching_operation_marker_does_not_bind_a_matching_name() {
    let spec = spec();
    let mut other = worker(&spec);
    other["env"] = json!({"HORIZON_CLOUD_OPERATION":"another-operation"});
    let (provider, _, task) = server(vec![(200, json!([other]).to_string())]);
    let mut state = CreateState::Requested;
    assert!(matches!(
        provider.reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(())),
        Err(CloudError::IdentityMismatch)
    ));
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
}

#[test]
fn inactive_matches_bind_identity_without_confirming_cleanup() {
    for status in ["TERMINATED", "EXITED", "UNKNOWN"] {
        let spec = spec();
        let mut inactive = worker(&spec);
        inactive["desiredStatus"] = json!(status);
        inactive["lastStatusChange"] = json!("Terminated by User");
        let (provider, requests, task) = server(vec![
            (200, json!([inactive.clone()]).to_string()),
            (200, inactive.to_string()),
            (404, "{}".into()),
            (404, "{}".into()),
        ]);
        let mut state = CreateState::Requested;
        let report = provider
            .reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        assert_eq!(
            report.outcome,
            Outcome::Inactive {
                worker_id: "worker1".into()
            }
        );
        assert!(report.worker.is_some());
        assert!(matches!(state, CreateState::Bound { .. }));
        assert!(matches!(
            provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
            Err(CloudError::Invalid(
                "Existing worker is not running; check provider before reconnecting"
            ))
        ));
        let missing = provider
            .reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        assert_eq!(
            missing.outcome,
            Outcome::Missing {
                worker_id: "worker1".into()
            }
        );
        assert!(matches!(state, CreateState::Bound { .. }));
        provider
            .terminate(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        assert!(matches!(state, CreateState::Terminated { .. }));
        task.join().unwrap();
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
    }
}

#[test]
fn explicit_termination_is_permanent_without_provider_access() {
    let provider = RunPod::new(Credential::new("unused-test-key".into()).unwrap());
    let mut state = CreateState::Terminated {
        worker_id: "worker1".into(),
    };
    let report = provider
        .reconcile(&spec(), &mut state, None, &Cancellation::default(), |_| {
            panic!("Confirmed termination must stay permanent")
        })
        .unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Terminated {
            worker_id: "worker1".into()
        }
    );
    assert!(report.worker.is_none());
}

#[test]
fn failed_persistence_keeps_requested_identity() {
    let spec = spec();
    let (provider, _, task) = server(vec![(200, json!([worker(&spec)]).to_string())]);
    let mut state = CreateState::Requested;
    assert!(matches!(
        provider.reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Err(
            CloudError::Persistence
        )),
        Err(CloudError::Persistence)
    ));
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
}

#[test]
fn lookup_refusals_do_not_clear_an_existing_create_fence() {
    for status in [400, 401, 403, 404, 422, 429, 503] {
        let (provider, _, task) = server(vec![(status, "PRIVATE BODY".into())]);
        let mut state = CreateState::Requested;
        let error = provider
            .reconcile(&spec(), &mut state, None, &Cancellation::default(), |_| Ok(()))
            .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE"));
        assert_eq!(state, CreateState::Requested);
        task.join().unwrap();
    }
}

#[test]
fn cancellation_and_invalid_hints_do_not_access_provider() {
    let provider = RunPod::new(Credential::new("unused-test-key".into()).unwrap());
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut state = CreateState::Requested;
    assert!(matches!(
        provider.reconcile(&spec(), &mut state, None, &cancel, |_| Ok(())),
        Err(CloudError::Cancelled)
    ));
    for hint in ["", "../other", "https://other.invalid", "worker?token=secret"] {
        assert!(matches!(
            provider.reconcile(&spec(), &mut state, Some(hint), &Cancellation::default(), |_| Ok(())),
            Err(CloudError::Invalid(_))
        ));
    }
    assert_eq!(state, CreateState::Requested);
    let mut prepared = CreateState::Prepared;
    assert_eq!(
        provider
            .reconcile(&spec(), &mut prepared, None, &Cancellation::default(), |_| Ok(()))
            .unwrap()
            .outcome,
        Outcome::Prepared
    );
    assert!(matches!(
        provider.reconcile(
            &spec(),
            &mut prepared,
            Some("worker1"),
            &Cancellation::default(),
            |_| Ok(())
        ),
        Err(CloudError::Invalid(_))
    ));
}
