use super::*;

#[test]
fn settled_claim_survives_source_owner_migration_and_does_not_relock_itself() {
    let mut fixture = Fixture::new();
    let op = fixture.submit(Action::Stop);
    execute_with(&fixture.request(), op.intent.operation_id, &mut Fake::new(&fixture)).unwrap();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.clear();
    assert!(state.intents.retire(&state.owner).unwrap());
    fixture.owner.scope.workspace_id = "moved".into();
    state.owner = fixture.owner.clone();
    store.save(&state).unwrap();
    drop(store);
    fixture.context.source.scope = fixture.owner.scope.clone();
    fixture.context.inventory[0].scope = fixture.owner.scope.clone();
    fixture.binding = Binding::new(
        &fixture.owner,
        "consumer",
        fixture.context.inventory[0].clone(),
        fixture.binding.checkout().into(),
        Origin::Existing,
    )
    .unwrap();
    fixture.select(&fixture.owner);
    bind(&fixture.request(), fixture.binding.clone()).unwrap();
    assert_eq!(fixture.submit(Action::EnsureReady).phase, Phase::Submitted);
}

#[test]
fn a_declined_creation_or_unclaimed_submission_can_be_cancelled_but_uncertainty_cannot() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    std::fs::remove_file(fixture.root.path().join("target/companion-operation.json")).unwrap();
    cancel_submission(&fixture.request(), op.intent.operation_id).unwrap();
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Failed
    );
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    backend.fail = true;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert!(cancel_submission(&fixture.request(), op.intent.operation_id).is_err());
}

#[test]
fn a_live_executor_cannot_be_reentered_during_access_verification() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    let held = receipt::execution_lock(&fixture.root.path().join("target")).unwrap();
    let mut backend = Fake::new(&fixture);
    assert!(matches!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend),
        Err(Error::Busy)
    ));
    assert!(backend.decisions.is_empty());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().phase,
        Phase::Submitted
    );
    drop(held);
    execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
}

#[test]
fn released_hetzner_grant_moves_only_after_provider_proof_and_keeps_its_worktree_identity() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    let (source, mut state) = fixture.request().load().unwrap();
    let grant = state.grants.get_mut("consumer").unwrap();
    grant.target_worker = Some("old-server".into());
    grant.revision = Some("a".repeat(40));
    grant.target_revoked = false;
    source.save(&state).unwrap();
    drop(source);
    let mut deployed = fixture.ready();
    deployed.profile.provider = "hetzner".into();
    deployed.worker.as_mut().unwrap().id = "new-server".into();
    deployed.operation = CreateState::Bound {
        worker_id: "new-server".into(),
    };
    fixture.save(&deployed);
    std::fs::write(fixture.root.path().join("target/hetzner.json"), b"{}").unwrap();
    execution::renew_released_grant(&fixture.request()).unwrap();
    assert_eq!(
        fixture.request().load().unwrap().1.grants["consumer"]
            .target_worker
            .as_deref(),
        Some("old-server")
    );
    let mut released = deployed;
    released.stage = Stage::Stopped;
    released.stop_requested = true;
    released.worker = None;
    released.operation = CreateState::Bound {
        worker_id: "old-server".into(),
    };
    let checked = crate::cloud_runtime::lifecycle::ReconciledDeployment {
        state: released,
        report: horizon_cloud::runpod::recovery::Reconciliation {
            operation_id: "target".into(),
            outcome: horizon_cloud::runpod::recovery::Outcome::Inactive {
                worker_id: "old-server".into(),
            },
            worker: None,
        },
    };
    let target = fixture.request().target_store(&fixture.binding).unwrap();
    receipt::released(&target, &checked).unwrap();
    drop(target);
    execution::renew_released_grant(&fixture.request()).unwrap();
    let (_, saved) = fixture.request().load().unwrap();
    let grant = &saved.grants["consumer"];
    assert_eq!(grant.target_worker.as_deref(), Some("new-server"));
    assert_eq!(grant.id, "grant");
    assert_eq!(grant.revision.as_deref(), Some("a".repeat(40).as_str()));
    assert!(grant.target_revoked && grant.access.is_none());
    assert_eq!(
        saved.intents.operation(op.intent.operation_id).unwrap().state,
        State::Submitted,
        "changing the pin alone is not verified readiness"
    );
}

#[test]
fn runpod_deleting_or_deleted_storage_refuses_the_live_backend_without_credentials() {
    for storage_state in ["deleting", "deleted"] {
        let fixture = Fixture::new();
        let mut state = fixture.ready();
        let spec: horizon_cloud::WorkerSpec = serde_json::from_value(serde_json::json!({
            "operation_id":"target", "image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":state.profile,"public_key":"synthetic","registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        })).unwrap();
        state.spec = Some(spec.clone());
        fixture.save(&state);
        std::fs::write(fixture.root.path().join("target/workspace-volume.json"), serde_json::to_vec(&serde_json::json!({
            "version":1,"worker":spec,"spec":{"operation_id":"target","size":state.profile.storage.volume_gb,"data_center_id":"dc1"},
            "state":{"state":storage_state,"volume":{"id":"volume1","name":"horizon-volume-target","size":state.profile.storage.volume_gb,"dataCenterId":"dc1"}}
        })).unwrap()).unwrap();
        let settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"/missing", "ssh_identity_file":"/missing", "docker_config":"/missing", "cpu_flavors":[],"gpu_types":[]
        })).unwrap();
        let op = fixture.submit(Action::EnsureReady);
        let result = execute(
            &fixture.request(),
            op.intent.operation_id,
            &settings,
            &Cancellation::default(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(result.phase, Phase::Refused);
    }
}

#[test]
fn a_crash_with_no_unfinished_provider_work_allows_stop_without_replaying_the_old_request() {
    for phase in [
        Phase::VerifyingAccess,
        Phase::Settling,
        Phase::Submitted,
        Phase::ConfirmationRequired,
    ] {
        let fixture = Fixture::new();
        let op = fixture.submit(Action::EnsureReady);
        let (source, mut state) = fixture.request().load().unwrap();
        state
            .intents
            .transition(op.intent.operation_id, State::Executing)
            .unwrap();
        source.save(&state).unwrap();
        drop(source);
        let target = fixture.request().target_store(&fixture.binding).unwrap();
        receipt::save(&target, &fixture.owner, op.intent.operation_id, phase).unwrap();
        drop(target);
        let mut failed_provider = Fake::new(&fixture);
        failed_provider.fail = true;
        assert_eq!(
            execute_with(&fixture.request(), op.intent.operation_id, &mut failed_provider)
                .unwrap()
                .phase,
            Phase::RetryRequired
        );
        assert!(
            failed_provider.decisions.is_empty(),
            "do not erase completed-provider proof by replaying recovery"
        );
        let stop = fixture.submit(Action::Stop);
        assert_eq!(stop.phase, Phase::Submitted);
        let old = status(&fixture.request(), op.intent.operation_id).unwrap();
        assert_eq!(old.intent.state, State::RetryRequired);
        assert_eq!(
            old.phase,
            Phase::RetryRequired,
            "terminal reason survives replacement of the target receipt"
        );
        let mut backend = Fake::new(&fixture);
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
        assert!(backend.decisions.is_empty());
    }
}

#[test]
fn missing_reserved_targets_require_confirmation_and_existing_ids_cannot_bypass_selection() {
    let fixture = Fixture::new();
    let mut target = fixture.binding.target().clone();
    target.cloud_id = "reserved".into();
    let binding = Binding::new(
        &fixture.owner,
        "consumer",
        target,
        fixture.binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    let (store, mut state) = fixture.request().load().unwrap();
    state.intents = intent::Journal::default();
    state.grants.clear();
    store.save(&state).unwrap();
    drop(store);
    let invalid = Binding::new(
        &fixture.owner,
        "consumer",
        fixture.binding.target().clone(),
        fixture.binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    assert!(bind(&fixture.request(), invalid).is_err());
    bind(&fixture.request(), binding).unwrap();
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend)
            .unwrap()
            .phase,
        Phase::ConfirmationRequired
    );
    assert!(backend.decisions.is_empty());
    cancel_submission(&fixture.request(), op.intent.operation_id).unwrap();
    // Once its operation settled, the reserved identity needs the owner's selection too.
    let refused = submit(&fixture.request(), Action::Stop, OperationId::generate()).unwrap_err();
    assert!(refused.to_string().contains("no longer selected"), "{refused}");
}

#[test]
fn cancellation_before_io_and_read_only_checks_do_not_fence_new_actions() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":"/missing", "ssh_identity_file":"/missing", "docker_config":"/missing", "cpu_flavors":[],"gpu_types":[]
    })).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(execute(&fixture.request(), op.intent.operation_id, &settings, &cancel, &|_| {}).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Failed
    );
    let stop = fixture.submit(Action::Stop);
    let mut state = fixture.ready();
    state.operation = CreateState::Requested;
    fixture.save(&state);
    let result = execute_with(&fixture.request(), stop.intent.operation_id, &mut Fake::new(&fixture)).unwrap();
    assert_eq!(result.intent.state, State::RetryRequired);
    assert_eq!(fixture.submit(Action::EnsureReady).phase, Phase::Submitted);
}

#[test]
fn missing_ssh_identity_fails_before_reconnect_or_resume_and_allows_explicit_stop() {
    for stage in [Stage::Readiness, Stage::Stopped] {
        let fixture = Fixture::new();
        let mut state = fixture.ready();
        state.stage = stage;
        state.stop_requested = stage == Stage::Stopped;
        fixture.save(&state);
        let credential = tempfile::NamedTempFile::new_in(fixture.root.path()).unwrap();
        std::fs::write(credential.path(), "synthetic-token").unwrap();
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":credential.path(), "ssh_identity_file":fixture.root.path().join("missing"),
            "docker_config":"/missing", "cpu_flavors":[], "gpu_types":[]
        }))
        .unwrap();
        settings.credential().unwrap();
        let op = fixture.submit(Action::EnsureReady);
        let error = execute(
            &fixture.request(),
            op.intent.operation_id,
            &settings,
            &Cancellation::default(),
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, Error::Io(_)));
        assert_eq!(
            status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
            State::Failed
        );
        assert_eq!(fixture.submit(Action::Stop).phase, Phase::Submitted);
        assert_eq!(
            fixture
                .request()
                .target_store(&fixture.binding)
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .stage,
            stage
        );
    }
}

#[test]
fn the_live_backend_reports_no_pending_mutation_for_a_run_that_fails_before_one() {
    use execution::Backend as _;
    let fixture = Fixture::new();
    fixture.save(&fixture.ready());
    // No RunPod key and no SSH identity: every path fails before a provider mutation.
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":"/missing", "ssh_identity_file":"/missing", "docker_config":"/missing", "cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let store = Store::lock(&fixture.root.path().join("target")).unwrap();
    for (decision, action) in [
        (Decision::Stop, Action::Stop),
        (Decision::Reconnect, Action::EnsureReady),
    ] {
        let mut live = execution::Live {
            settings: &settings,
            cancel: &Cancellation::default(),
            emit: &|_| {},
            pending: true,
            inspecting: false,
            settled: false,
        };
        assert!(live.run(&store, decision, action).is_err());
        assert!(!live.mutation_uncertain(), "{decision:?} sent nothing");
    }
}

#[test]
fn reconciling_an_uncertain_run_that_fails_keeps_it_uncertain() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.stage = Stage::Readiness;
    fixture.save(&state);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    backend.fail = true;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Uncertain
    );
    // The reconciliation itself fails, and reports no mutation of its own.
    backend.uncertain = false;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(backend.decisions, [Decision::Reconnect, Decision::ReconcileOnly]);
    assert_eq!(
        backend.inspected,
        [false, false],
        "a re-entered operation is no inspection"
    );
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Uncertain,
        "the original outcome is still unknown"
    );
}

#[test]
fn stopping_a_workerless_prepared_record_marks_nothing() {
    use execution::Backend as _;
    let fixture = Fixture::new();
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":"/missing", "ssh_identity_file":"/missing", "docker_config":"/missing", "cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    for (operation, marked) in [
        (CreateState::Prepared, false),
        (
            CreateState::Bound {
                worker_id: "worker1".into(),
            },
            true,
        ),
    ] {
        let mut state = fixture.ready();
        state.operation = operation;
        state.stop_requested = false;
        fixture.save(&state);
        let store = Store::lock(&fixture.root.path().join("target")).unwrap();
        let mut live = execution::Live {
            settings: &settings,
            cancel: &Cancellation::default(),
            emit: &|_| {},
            pending: false,
            inspecting: false,
            settled: false,
        };
        assert_eq!(
            live.run(&store, Decision::AlreadyStopped, Action::Stop).unwrap(),
            Phase::Stopped
        );
        assert_eq!(store.load().unwrap().unwrap().stop_requested, marked);
    }
}

#[test]
fn only_a_settled_resume_lets_reconciliation_run_its_reconnect() {
    use crate::cloud_runtime::lifecycle::ReconciledDeployment;
    use horizon_cloud::runpod::recovery::{Outcome, Reconciliation};
    let fixture = Fixture::new();
    let store = Store::lock(&fixture.root.path().join("target")).unwrap();
    let reconciled = |state: Deployment| ReconciledDeployment {
        state,
        report: Reconciliation {
            operation_id: "target".into(),
            outcome: Outcome::Prepared,
            worker: None,
        },
    };
    let mut prepared = fixture.ready();
    prepared.operation = CreateState::Prepared;
    prepared.worker = None;
    prepared.stage = Stage::Readiness;
    // No journal: nothing proves a server ever held a volume.
    assert!(!execution::resume_settled(&store, &reconciled(prepared.clone())).unwrap());
    let held = serde_json::json!({"location": "hel1", "volume": {"state": "bound", "worker_id": "9"}});
    std::fs::write(store.root().join("hetzner.json"), held.to_string()).unwrap();
    // A stale Hetzner journal proves nothing for a provider that keeps its worker.
    assert!(!execution::resume_settled(&store, &reconciled(prepared.clone())).unwrap());
    prepared.profile.provider = horizon_cloud::hetzner::PROVIDER.into();
    assert!(execution::resume_settled(&store, &reconciled(prepared.clone())).unwrap());
    // A stop requested since means the resume did not settle.
    prepared.stop_requested = true;
    assert!(!execution::resume_settled(&store, &reconciled(prepared)).unwrap());
}

#[test]
fn an_inspection_that_left_a_mutation_pending_stays_uncertain() {
    for (uncertain, expected) in [(true, State::Uncertain), (false, State::RetryRequired)] {
        let fixture = Fixture::new();
        let mut state = fixture.ready();
        // No recorded worker: a submitted request only inspects, and may reconnect.
        state.worker = None;
        fixture.save(&state);
        let op = fixture.submit(Action::EnsureReady);
        let mut backend = Fake::new(&fixture);
        backend.fail = true;
        backend.uncertain = uncertain;
        assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
        assert_eq!(backend.decisions, [Decision::ReconcileOnly]);
        assert_eq!(backend.inspected, [true], "the backend is told it only inspects");
        assert_eq!(
            status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
            expected,
            "uncertain: {uncertain}"
        );
    }
}

#[test]
fn a_reconnect_after_a_proven_resume_that_fails_without_a_mutation_can_be_retried() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.stage = Stage::Readiness;
    fixture.save(&state);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    backend.fail = true;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Uncertain
    );
    // Recovery proves the resume settled, reconnects, and fails before any mutation.
    backend.uncertain = false;
    backend.settled = true;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::RetryRequired
    );
}

#[test]
fn an_ensure_interrupted_before_its_resume_continues_it_once_the_stop_is_confirmed() {
    use crate::cloud_runtime::lifecycle::ReconciledDeployment;
    use horizon_cloud::runpod::recovery::{Outcome, Reconciliation};
    let fixture = Fixture::new();
    let reconciled = |state: Deployment, outcome: Outcome| ReconciledDeployment {
        state,
        report: Reconciliation {
            operation_id: "target".into(),
            outcome,
            worker: None,
        },
    };
    // A Hetzner stop released the server, which the check proves gone.
    let mut released = fixture.ready();
    released.profile.provider = horizon_cloud::hetzner::PROVIDER.into();
    released.stage = Stage::Stopped;
    released.stop_requested = true;
    let inactive = Outcome::Inactive {
        worker_id: "worker1".into(),
    };
    assert_eq!(
        execution::resume_to_continue(&reconciled(released.clone(), inactive)),
        Some(Decision::ResumeWithNewServer)
    );
    // Not yet proven stopped: the earlier operation stays unresolved.
    let found = Outcome::Found {
        worker_id: "worker1".into(),
    };
    assert_eq!(
        execution::resume_to_continue(&reconciled(released.clone(), found)),
        None
    );
    released.stage = Stage::Stopping;
    let inactive = Outcome::Inactive {
        worker_id: "worker1".into(),
    };
    assert_eq!(execution::resume_to_continue(&reconciled(released, inactive)), None);
}
