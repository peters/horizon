use super::*;

#[test]
fn new_start_records_identity_and_intent_before_factory_then_retains_complete_trust() {
    let f = Fixture::new();
    let saved = f
        .start_using(i64::MAX, |trust| {
            assert!(matches!(&trust, TrustSelection::Initial(_)));
            let allocation = f.allocation();
            let request = allocation.worker_request().expect("reserved request");
            f.identities
                .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
                .expect("durable key");
            assert_eq!(
                f.store.load_remote_first_pin_request(&allocation).expect("intent"),
                Some(request)
            );
            assert_eq!(f.counts(), [1, 1, 0, 1]);
            f.factory(&trust)
        })
        .expect("start");
    assert_eq!(f.counts(), [1, 1, 1, 0]);
    assert_eq!(
        saved.workspace().state().runtime.as_ref().expect("runtime").phase,
        RemoteRuntimePhase::Reconciling
    );
    assert!(saved.workspace().state().spec.panels.is_empty());
    let before = f.snapshot();
    for operation in [Operation::Retry, Operation::Recover] {
        assert_eq!(f.run(operation).expect("retained"), saved);
    }
    assert_eq!(f.snapshot(), before);
    assert_eq!(*f.remote.selections.lock().expect("selections"), [1, 2]);
    assert_eq!(*f.remote.calls.lock().expect("calls"), [1, 1, 0, 2, 0]);
}

#[test]
fn provisioning_and_lost_response_recover_after_controller_restart_without_ensure() {
    for lost in [false, true] {
        let f = Fixture::new();
        f.remote.provisioning.store(!lost, Ordering::SeqCst);
        f.remote.lose_response.store(lost, Ordering::SeqCst);
        let first = f.start();
        if lost {
            let error = first.expect_err("lost response");
            assert_eq!(error, RemoteWorkspaceSetupError::ProviderUnavailable.into());
            assert!(!format!("{error:?} {error}").contains("private-provider-payload"));
        } else {
            first.expect("provisioning");
        }
        let before = f.snapshot();
        assert_eq!(before.counts, [1, 1, 1, 1]);
        *f.remote.observation.lock().expect("observation") = Some(f.status());
        let reopened = CloudWorkflowStore::open_path(f.store.path()).expect("new controller store");
        let saved = dispatch(
            &reopened,
            &f.identities,
            &f.key,
            &f.profile,
            &before.allocation,
            Operation::Retry,
            |trust| f.factory(&trust),
        )
        .expect("recover initial trust");
        assert_eq!(
            saved.worker_request().expect("request"),
            before.allocation.worker_request().expect("old request")
        );
        assert_eq!(saved.workflow(), before.allocation.workflow());
        assert_eq!(f.keys(), before.keys);
        assert_eq!(f.counts(), [1, 1, 1, 0]);
        assert_eq!(
            *f.remote.calls.lock().expect("calls"),
            if lost { [1, 1, 1, 0, 0] } else { [1, 1, 0, 1, 0] }
        );
        f.run(Operation::Recover).expect("retained trust");
        assert_eq!(*f.remote.selections.lock().expect("selections"), [2, 1]);
    }
}

#[test]
fn recovery_and_consumed_claim_absence_never_reopen_creation() {
    for claimed in [false, true] {
        let f = Fixture::new();
        let saved = f.reserve(true);
        if claimed {
            f.claim(&saved);
        }
        let keys = f.keys();
        f.run(if claimed { Operation::Retry } else { Operation::Recover })
            .expect("absence");
        f.run(Operation::Retry).expect("repeated absence");
        assert_eq!(f.counts(), [1, 1, i64::from(claimed), 1]);
        assert_eq!(f.keys(), keys);
        assert_eq!(*f.remote.calls.lock().expect("calls"), [0, 0, 2, 0, 0]);
    }
}

#[test]
fn expired_setup_recovers_persistent_worker_but_expired_ready_lease_is_rejected() {
    let f = Fixture::new();
    f.reserve(true);
    let mut workflow = f.allocation().workflow().workflow().clone();
    workflow.created_at_millis = 1000;
    workflow.updated_at_millis = 1000;
    workflow.retain_until_millis = 2000;
    rusqlite::Connection::open(f.store.path()).expect("database").execute(
        "UPDATE cloud_workflows SET created_at_millis=1000,updated_at_millis=1000,retain_until_millis=2000,snapshot=?1 WHERE workflow_id=?2",
        rusqlite::params![serde_json::to_vec(&workflow).expect("snapshot"), workflow.id.to_string()],
    ).expect("expire fixture");
    *f.remote.observation.lock().expect("observation") = Some(f.status());
    f.run(Operation::Retry).expect("persistent recovery");
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0, 0, 1, 0, 0]);

    let mut f = Fixture::new();
    let mut state = f.dormant.state().clone();
    state.spec.target.lifetime = crate::cloud_run::WorkerLifetime::TimeLimited { seconds: 600 };
    f.dormant = f
        .store
        .replace_remote_workspace(&f.dormant, &state)
        .expect("timed fixture");
    f.reserve(true);
    let mut observed = f.status();
    observed.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: "2000-01-01T00:00:00Z".into(),
    });
    *f.remote.observation.lock().expect("observation") = Some(observed);
    let before = f.snapshot();
    assert_eq!(
        f.run(Operation::Recover),
        Err(RemoteWorkspaceRecoveryError::InvalidObservation.into())
    );
    assert_eq!(f.snapshot(), before);
}
