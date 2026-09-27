use super::*;
use crate::cloud_runtime::{CreateState, Stage, state::Deployment};
use horizon_cloud::companions::{Placement, Scope};

fn owner() -> Owner {
    Owner {
        scope: Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        },
        cloud_id: "source".into(),
    }
}
fn binding(alias: &str, origin: Origin) -> Binding {
    Binding::new(
        &owner(),
        alias,
        Target {
            scope: owner().scope,
            cloud_id: "target".into(),
            declaration: Declaration::new("example/consumer", "cpu"),
        },
        std::env::temp_dir().join("synthetic-consumer"),
        origin,
    )
    .unwrap()
}
fn ready() -> Deployment {
    serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"target","repository":binding("consumer", Origin::Existing).checkout(),"revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"sessions":[],
        "source_ready":true,
        "worker":{"id":"worker1","name":"w","imageName":"i","desiredStatus":"RUNNING",
            "publicIp":"203.0.113.7","portMappings":{"22":22022}}
    }))
    .unwrap()
}
fn ensure(state: &Deployment) -> Decision {
    decide(
        Action::EnsureReady,
        &binding("consumer", Origin::Existing),
        Observation::Existing(state),
        State::Submitted,
        true,
    )
}
fn journal() -> Journal {
    let mut journal = Journal::default();
    journal
        .bind(&owner(), "consumer", binding("consumer", Origin::Reserved))
        .unwrap();
    journal
}

#[test]
fn binding_is_passive_local_and_pins_identity_before_inventory_exists() {
    let journal = journal();
    assert!(journal.intents.is_empty());
    let bound = journal.binding("consumer").unwrap();
    assert_eq!(bound.origin(), Origin::Reserved);
    assert_eq!(bound.target().cloud_id, "target");
    bound
        .validate(&owner(), "consumer", &Declaration::new("example/consumer", "cpu"))
        .unwrap();
    for declaration in [
        Declaration::new("example/other", "cpu"),
        Declaration::new("example/consumer", "gpu"),
    ] {
        assert!(bound.validate(&owner(), "consumer", &declaration).is_err());
    }
    assert!(
        bound
            .validate(&owner(), "other-alias", &bound.target.declaration)
            .is_err()
    );
    let mut moved = owner();
    moved.scope.workspace_id = "another-workspace".into();
    assert!(journal.validate(&moved).is_err());
}

#[test]
fn invalid_and_same_worker_bindings_are_refused() {
    let valid = binding("consumer", Origin::Reserved);
    let make = |target, path| Binding::new(&owner(), "consumer", target, path, Origin::Reserved);
    assert!(make(valid.target.clone(), "relative/path".into()).is_err());
    let mut sibling = valid.target.clone();
    sibling.declaration.placement = Placement::SameWorker;
    assert!(make(sibling, valid.checkout.clone()).is_err());
    let mut cross_workspace = valid.target.clone();
    cross_workspace.scope.workspace_id = "another-workspace".into();
    assert!(make(cross_workspace, valid.checkout.clone()).is_err());
    let mut self_target = valid.target;
    self_target.cloud_id = owner().cloud_id;
    assert!(make(self_target, valid.checkout).is_err());
}

#[test]
fn explicit_binding_never_guesses_or_silently_rebinds() {
    let mut journal = journal();
    let mut other = binding("consumer", Origin::Reserved);
    other.target.cloud_id = "another-cloud".into();
    assert!(journal.bind(&owner(), "consumer", other).is_err());
    let mut same_target = binding("alternate", Origin::Reserved);
    same_target.checkout = std::env::temp_dir().join("different-checkout");
    assert!(journal.bind(&owner(), "alternate", same_target).is_err());
}

#[test]
fn ready_requires_both_the_worker_environment_and_source_grant() {
    let mut state = ready();
    assert_eq!(ensure(&state), Decision::Reuse);
    assert_eq!(
        decide(
            Action::EnsureReady,
            &binding("consumer", Origin::Existing),
            Observation::Existing(&state),
            State::Submitted,
            false
        ),
        Decision::VerifyAccess
    );
    state.source_ready = false;
    assert_eq!(ensure(&state), Decision::Reconnect);
    state.stage = Stage::Readiness;
    assert_eq!(ensure(&state), Decision::Reconnect);
    state.worker.as_mut().unwrap().port_mappings = None;
    assert_eq!(ensure(&state), Decision::Reconnect);
}

#[test]
fn stopped_is_sticky_and_only_explicit_ensure_plans_resume() {
    let mut state = ready();
    state.stop_requested = true;
    assert_eq!(ensure(&state), Decision::ResumeThenReconnect);
    state.stage = Stage::Stopped;
    state.worker.as_mut().unwrap().desired_status = "EXITED".into();
    assert_eq!(ensure(&state), Decision::ResumeThenReconnect);
    assert_eq!(
        decide(
            Action::Stop,
            &binding("consumer", Origin::Existing),
            Observation::Existing(&state),
            State::Submitted,
            false
        ),
        Decision::AlreadyStopped
    );
    assert!(state.stop_requested);
    state.profile.provider = horizon_cloud::hetzner::PROVIDER.into();
    state.worker = None;
    assert_eq!(ensure(&state), Decision::ResumeWithNewServer);
}

#[test]
fn every_deleted_or_terminated_record_is_refused_before_any_other_plan() {
    for stage in [
        Stage::Deleted,
        Stage::ReleaseDevices,
        Stage::DeleteWorker,
        Stage::DeleteStorage,
    ] {
        let mut state = ready();
        state.stage = stage;
        assert!(refuses_deployment(&state));
        for action in [Action::EnsureReady, Action::Stop] {
            for intent in [State::Submitted, State::Executing, State::Uncertain] {
                assert_eq!(
                    decide(
                        action,
                        &binding("consumer", Origin::Existing),
                        Observation::Existing(&state),
                        intent,
                        true
                    ),
                    Decision::Refuse(Refusal::Deleted)
                );
            }
        }
    }
    let mut terminated = ready();
    terminated.operation = CreateState::Terminated {
        worker_id: "worker1".into(),
    };
    assert_eq!(ensure(&terminated), Decision::Refuse(Refusal::Deleted));
}

#[test]
fn uncertain_allocation_stop_and_replacement_only_reconcile() {
    let mut state = ready();
    state.operation = CreateState::Requested;
    assert_eq!(ensure(&state), Decision::ReconcileOnly);
    for stage in [Stage::Stopping, Stage::Replace] {
        let mut state = ready();
        state.stage = stage;
        assert_eq!(ensure(&state), Decision::ReconcileOnly);
    }
    let state = ready();
    for intent in [State::Executing, State::Uncertain, State::Succeeded, State::Failed] {
        assert_eq!(
            decide(
                Action::EnsureReady,
                &binding("consumer", Origin::Existing),
                Observation::Existing(&state),
                intent,
                true
            ),
            Decision::ReconcileOnly
        );
    }
}

#[test]
fn missing_reserved_target_can_be_provisioned_but_existing_or_lost_cannot() {
    let mut bound = binding("consumer", Origin::Reserved);
    assert_eq!(
        decide(
            Action::EnsureReady,
            &bound,
            Observation::Missing,
            State::Submitted,
            false
        ),
        Decision::Provision
    );
    assert_eq!(
        decide(
            Action::EnsureReady,
            &bound,
            Observation::Missing,
            State::Executing,
            false
        ),
        Decision::ReconcileOnly
    );
    assert_eq!(
        decide(Action::Stop, &bound, Observation::Missing, State::Submitted, false),
        Decision::AlreadyStopped
    );
    assert_eq!(
        decide(Action::EnsureReady, &bound, Observation::Lost, State::Submitted, false),
        Decision::Refuse(Refusal::Lost)
    );
    bound.mark_existing();
    assert_eq!(
        decide(
            Action::EnsureReady,
            &bound,
            Observation::Missing,
            State::Submitted,
            false
        ),
        Decision::Refuse(Refusal::Lost)
    );
    let mut state = ready();
    state.worker = None;
    assert_eq!(ensure(&state), Decision::ReconcileOnly);
    state.worker = ready().worker;
    state.worker.as_mut().unwrap().desired_status = "TERMINATED".into();
    assert_eq!(ensure(&state), Decision::Refuse(Refusal::Lost));
}

#[test]
fn prepared_deployment_reconnects_and_wrong_identity_never_executes() {
    let mut state = ready();
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.stage = Stage::Validate;
    assert_eq!(ensure(&state), Decision::Reconnect);
    state.cloud_id = "different-cloud".into();
    assert_eq!(ensure(&state), Decision::Refuse(Refusal::IdentityMismatch));
    let mut state = ready();
    state.worker.as_mut().unwrap().id = "different-worker".into();
    assert_eq!(ensure(&state), Decision::Refuse(Refusal::IdentityMismatch));
    let mut state = ready();
    state.repository = std::env::temp_dir().join("another-repository");
    assert_eq!(ensure(&state), Decision::Refuse(Refusal::IdentityMismatch));
    assert_eq!(
        decide(
            Action::Stop,
            &binding("consumer", Origin::Existing),
            Observation::Existing(&ready()),
            State::Submitted,
            true
        ),
        Decision::Stop
    );
}

#[test]
fn requests_deduplicate_by_target_and_opposite_actions_cannot_overwrite_pending_work() {
    let mut journal = journal();
    journal
        .bind(&owner(), "alternate", binding("alternate", Origin::Reserved))
        .unwrap();
    let first = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    assert_eq!(
        journal
            .submit("consumer", Action::EnsureReady, OperationId::generate())
            .unwrap(),
        first
    );
    assert_eq!(
        journal
            .submit("alternate", Action::EnsureReady, OperationId::generate())
            .unwrap(),
        first
    );
    assert!(matches!(
        journal.submit("alternate", Action::Stop, OperationId::generate()),
        Err(Error::Busy)
    ));
    journal.transition(first.operation_id, State::Executing).unwrap();
    journal.transition(first.operation_id, State::Uncertain).unwrap();
    assert!(journal.transition(first.operation_id, State::Submitted).is_err());
    journal.validate(&owner()).unwrap();
    journal.transition(first.operation_id, State::Succeeded).unwrap();
    assert!(journal.transition(first.operation_id, State::Executing).is_err());
    assert!(journal.submit("consumer", Action::Stop, first.operation_id).is_err());
    let stop = journal
        .submit("consumer", Action::Stop, OperationId::generate())
        .unwrap();
    assert_eq!(stop.action, Action::Stop);
    assert_ne!(stop.operation_id, first.operation_id);
    assert!(
        journal
            .submit("unselected", Action::EnsureReady, OperationId::generate())
            .is_err()
    );
}

#[test]
fn restart_preserves_the_fence_id_and_reserved_identity() {
    let mut journal = journal();
    let first = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    journal.transition(first.operation_id, State::Executing).unwrap();
    let mut restored: Journal = serde_json::from_slice(&serde_json::to_vec(&journal).unwrap()).unwrap();
    restored.validate(&owner()).unwrap();
    let retried = restored
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    assert_eq!(retried.operation_id, first.operation_id);
    assert_eq!(retried.target_cloud_id, "target");
    assert_eq!(
        decide(
            retried.action,
            restored.binding("consumer").unwrap(),
            Observation::Missing,
            retried.state,
            false
        ),
        Decision::ReconcileOnly
    );
    restored.mark_existing("target");
    assert_eq!(restored.binding("consumer").unwrap().origin(), Origin::Existing);
}

#[test]
fn corrupt_journal_rejects_duplicates_and_mismatched_intents() {
    let mut journal = journal();
    journal
        .bind(&owner(), "alternate", binding("alternate", Origin::Reserved))
        .unwrap();
    let intent = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    journal.intents.insert("alternate".into(), intent.clone());
    assert!(journal.validate(&owner()).is_err());
    journal.intents.get_mut("alternate").unwrap().operation_id = OperationId::generate();
    assert!(
        journal.validate(&owner()).is_err(),
        "two active IDs cannot target one cloud"
    );
    journal.intents.remove("alternate");
    journal.intents.get_mut("consumer").unwrap().target_cloud_id = "wrong".into();
    assert!(journal.validate(&owner()).is_err());
}

#[cfg(unix)] // The companion store requires durable directory updates.
#[test]
fn durable_journal_roundtrip_preserves_legacy_encoding_and_intent_after_lock_release() {
    let temp = tempfile::tempdir().unwrap();
    let owner = owner();
    let store = super::super::journal::Store::open(temp.path(), &owner).unwrap();
    let mut state = store.load().unwrap();
    let before = serde_json::to_value(&state).unwrap();
    assert!(before.get("intents").is_none());
    store.save(&state).unwrap();
    assert_eq!(serde_json::to_value(store.load().unwrap()).unwrap(), before);
    assert!(matches!(
        super::super::journal::Store::open(temp.path(), &owner),
        Err(Error::Busy)
    ));
    state.intents = journal();
    let intent = state
        .intents
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    state.intents.transition(intent.operation_id, State::Executing).unwrap();
    store.save(&state).unwrap();
    drop(store);
    let restored = super::super::journal::Store::open(temp.path(), &owner)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(
        restored.intents.operation(intent.operation_id).unwrap().state,
        State::Executing
    );
}

#[test]
fn late_retry_of_finished_ensure_never_undoes_a_newer_stop() {
    let mut journal = journal();
    let ensure = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    journal.transition(ensure.operation_id, State::Executing).unwrap();
    journal.transition(ensure.operation_id, State::Succeeded).unwrap();
    let stop = journal
        .submit("consumer", Action::Stop, OperationId::generate())
        .unwrap();
    journal.transition(stop.operation_id, State::Executing).unwrap();
    journal.transition(stop.operation_id, State::Succeeded).unwrap();
    let mut restored: Journal = serde_json::from_slice(&serde_json::to_vec(&journal).unwrap()).unwrap();
    restored.validate(&owner()).unwrap();
    let retried = restored
        .submit("consumer", Action::EnsureReady, ensure.operation_id)
        .unwrap();
    assert_eq!(retried.state, State::Succeeded);
    assert_eq!(restored.operation(stop.operation_id).unwrap().state, State::Succeeded);
    assert!(restored.submit("consumer", Action::Stop, ensure.operation_id).is_err());
    assert_eq!(
        decide(
            retried.action,
            restored.binding("consumer").unwrap(),
            Observation::Missing,
            retried.state,
            false
        ),
        Decision::ReconcileOnly
    );
}

#[test]
fn full_history_refuses_new_work_without_evicting_retry_fences() {
    let mut journal = journal();
    let mut first = None;
    for _ in 0..257 {
        let intent = journal
            .submit("consumer", Action::EnsureReady, OperationId::generate())
            .unwrap();
        first.get_or_insert(intent.operation_id);
        journal.transition(intent.operation_id, State::Failed).unwrap();
    }
    assert!(
        journal
            .submit("consumer", Action::EnsureReady, OperationId::generate())
            .is_err()
    );
    let retried = journal.submit("consumer", Action::EnsureReady, first.unwrap()).unwrap();
    assert_eq!(retried.state, State::Failed);
    journal.validate(&owner()).unwrap();
}

#[test]
fn provider_deletion_fences_refuse_even_without_a_deployment_record() {
    for origin in [Origin::Reserved, Origin::Existing] {
        for action in [Action::EnsureReady, Action::Stop] {
            assert_eq!(
                decide(
                    action,
                    &binding("consumer", origin),
                    Observation::DeletionPending,
                    State::Submitted,
                    false
                ),
                Decision::Refuse(Refusal::Deleted)
            );
        }
    }
}

#[test]
fn lost_dedup_response_cannot_turn_a_retry_into_fresh_ensure_after_stop() {
    let mut journal = journal();
    let first = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    let retry_id = OperationId::generate();
    let duplicate = journal.submit("consumer", Action::EnsureReady, retry_id).unwrap();
    assert_eq!(duplicate.operation_id, first.operation_id);
    journal.transition(first.operation_id, State::Executing).unwrap();
    journal.transition(first.operation_id, State::Succeeded).unwrap();
    let stop = journal
        .submit("consumer", Action::Stop, OperationId::generate())
        .unwrap();
    journal.transition(stop.operation_id, State::Executing).unwrap();
    journal.transition(stop.operation_id, State::Succeeded).unwrap();
    let mut restored: Journal = serde_json::from_slice(&serde_json::to_vec(&journal).unwrap()).unwrap();
    restored.validate(&owner()).unwrap();
    let retried = restored.submit("consumer", Action::EnsureReady, retry_id).unwrap();
    assert_eq!(retried.operation_id, first.operation_id);
    assert_eq!(retried.state, State::Succeeded);
    assert_eq!(restored.operation(retry_id), restored.operation(first.operation_id));
}
