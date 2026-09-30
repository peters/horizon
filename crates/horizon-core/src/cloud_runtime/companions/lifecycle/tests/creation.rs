use super::*;

/// A fixture whose alias is bound to a reserved cloud ID that has no cloud yet.
fn reserved() -> (Fixture, OperationId) {
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
    bind(&fixture.request(), binding).unwrap();
    let id = fixture.submit(Action::EnsureReady).intent.operation_id;
    (fixture, id)
}

/// The owner's checkbox on the new cloud, once the card has created it.
fn select_reserved(fixture: &mut Fixture) {
    let mut target = fixture.binding.target().clone();
    target.cloud_id = "reserved".into();
    fixture.context.inventory = vec![target.clone()];
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.insert(
        "consumer".into(),
        journal::Grant {
            selection: Selection::new(&fixture.context.source, "consumer", &target).unwrap(),
            target,
            id: "grant".into(),
            selected: true,
            source_worker: None,
            target_worker: None,
            revision: None,
            source_disconnected: true,
            target_revoked: true,
            source_forgotten: true,
            access: None,
        },
    );
    store.save(&state).unwrap();
}

fn prepared(fixture: &Fixture) -> Deployment {
    let mut state = fixture.ready();
    state.cloud_id = "reserved".into();
    state.operation = CreateState::Prepared;
    state.stage = Stage::Readiness;
    state.worker = None;
    state.source_ready = false;
    state
}

fn save_reserved(fixture: &Fixture, state: &Deployment) {
    Store::lock(&fixture.root.path().join("reserved"))
        .unwrap()
        .save(state)
        .unwrap();
}

#[test]
fn only_the_owners_confirmation_lets_a_reserved_companion_allocate_its_first_worker() {
    let (mut fixture, id) = reserved();
    {
        let mut backend = Fake::new(&fixture);
        assert_eq!(
            execute_with(&fixture.request(), id, &mut backend).unwrap().phase,
            Phase::ConfirmationRequired
        );
        // Confirmation needs the card to have created and prepared the cloud first.
        assert!(confirm_creation(&fixture.request(), id).is_err());
        // A prepared record alone never proves the owner approved paid creation.
        save_reserved(&fixture, &prepared(&fixture));
        assert_eq!(
            execute_with(&fixture.request(), id, &mut backend).unwrap().phase,
            Phase::ConfirmationRequired
        );
        assert!(backend.decisions.is_empty());
    }
    // An unknown operation is never confirmed.
    assert!(confirm_creation(&fixture.request(), OperationId::generate()).is_err());
    // Without the owner's selection of the new cloud, access could never be verified.
    assert!(confirm_creation(&fixture.request(), id).is_err());
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    let mut backend = Fake::new(&fixture);
    let operation = execute_with(&fixture.request(), id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
    assert_eq!(operation.phase, Phase::Ready);
    // A finished operation is a status result; confirming it again is refused.
    assert!(confirm_creation(&fixture.request(), id).is_err());
}

#[test]
fn confirming_an_unstarted_operation_again_changes_nothing_and_runs_it_once() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    let claim = || std::fs::read(fixture.root.path().join("reserved/companion-operation.json")).unwrap();
    let first = claim();
    // The card's Retry confirms again after a start that could not begin.
    confirm_creation(&fixture.request(), id).unwrap();
    assert_eq!(claim(), first);
    let mut backend = Fake::new(&fixture);
    execute_with(&fixture.request(), id, &mut backend).unwrap();
    execute_with(&fixture.request(), id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
}

#[test]
fn a_confirmation_refuses_records_that_do_not_match_the_reserved_binding() {
    let (mut fixture, id) = reserved();
    select_reserved(&mut fixture);
    for change in 0..4 {
        let mut state = prepared(&fixture);
        match change {
            0 => state.repository = fixture.root.path().join("elsewhere"),
            1 => state.operation = CreateState::Requested,
            2 => state.stop_requested = true,
            _ => state.worker = fixture.ready().worker,
        }
        save_reserved(&fixture, &state);
        assert!(confirm_creation(&fixture.request(), id).is_err(), "change {change}");
    }
    let receipt = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
    assert_eq!(receipt.confirmed, None);
}

#[test]
fn existing_companions_and_stops_are_never_confirmed_for_creation() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    assert!(confirm_creation(&fixture.request(), op.intent.operation_id).is_err());
    let (mut fixture, id) = reserved();
    cancel_submission(&fixture.request(), id).unwrap();
    select_reserved(&mut fixture);
    let stop = fixture.submit(Action::Stop).intent.operation_id;
    save_reserved(&fixture, &prepared(&fixture));
    assert!(confirm_creation(&fixture.request(), stop).is_err());
}

#[test]
fn unchecking_after_the_confirmation_withdraws_it_before_any_allocation() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    {
        let mut backend = Fake::new(&fixture);
        assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
        assert!(backend.decisions.is_empty());
        assert_eq!(status(&fixture.request(), id).unwrap().intent.state, State::Submitted);
    }
    // Checking the companion again does not revive the withdrawn confirmation.
    select_reserved(&mut fixture);
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), id, &mut backend).unwrap().phase,
        Phase::ConfirmationRequired
    );
    assert!(backend.decisions.is_empty());
    confirm_creation(&fixture.request(), id).unwrap();
    execute_with(&fixture.request(), id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
}

#[test]
fn a_confirmation_never_carries_over_to_another_owners_claim() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    let target = Store::lock(&fixture.root.path().join("reserved")).unwrap();
    let mut other = fixture.owner.clone();
    other.cloud_id = "other-source".into();
    // Another source's claim reusing the operation ID starts unconfirmed.
    receipt::save(&target, &other, id, Phase::Submitted).unwrap();
    assert_eq!(receipt::load(target.root()).unwrap().unwrap().confirmed, None);
    // The same owner's claim keeps its confirmation across phase updates.
    receipt::confirm(&target, &fixture.owner, id, "grant").unwrap();
    receipt::save(&target, &fixture.owner, id, Phase::Running).unwrap();
    assert_eq!(receipt::load(target.root()).unwrap().unwrap().confirmed, Some(id));
}

#[test]
fn checking_the_companion_again_after_an_uncheck_needs_a_fresh_confirmation() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    // Unchecked and checked again with no execution in between: a new grant.
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.get_mut("consumer").unwrap().id = "grant-2".into();
    store.save(&state).unwrap();
    drop(store);
    {
        let mut backend = Fake::new(&fixture);
        assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
        assert!(backend.decisions.is_empty());
    }
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), id, &mut backend).unwrap().phase,
        Phase::ConfirmationRequired
    );
    confirm_creation(&fixture.request(), id).unwrap();
    execute_with(&fixture.request(), id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
}

#[test]
fn a_creation_that_failed_before_allocating_can_be_confirmed_again() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    {
        // A definite local failure: nothing reached the provider.
        let mut backend = Fake::new(&fixture);
        backend.fail = true;
        backend.uncertain = false;
        assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
    }
    assert_eq!(status(&fixture.request(), id).unwrap().phase, Phase::RetryRequired);
    // The binding now counts as existing, and its record is still unallocated.
    let retry = fixture.submit(Action::EnsureReady).intent.operation_id;
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), retry, &mut backend).unwrap().phase,
        Phase::ConfirmationRequired
    );
    assert!(backend.decisions.is_empty());
    confirm_creation(&fixture.request(), retry).unwrap();
    execute_with(&fixture.request(), retry, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
}

/// An unbound fixture and a fresh reserved identity for its companion.
fn fresh(cloud_id: &str) -> (Fixture, Binding) {
    let fixture = Fixture::unbound();
    let mut target = fixture.binding.target().clone();
    target.cloud_id = cloud_id.into();
    let binding = Binding::new(
        &fixture.owner,
        "consumer",
        target,
        fixture.binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    (fixture, binding)
}

#[test]
fn a_reservation_records_its_binding_and_operation_together_and_a_retry_changes_nothing() {
    let (fixture, binding) = fresh("reserved");
    let id = OperationId::generate();
    let first = reserve(&fixture.request(), binding.clone(), id).unwrap();
    assert_eq!(first.intent.target_cloud_id, "reserved");
    assert_eq!(first.phase, Phase::Submitted);
    assert_eq!(
        bound_checkout(&fixture.request()).unwrap().as_deref(),
        Some(binding.checkout())
    );
    let claim = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
    assert_eq!(claim.id, id);
    // The card's Retry reserves again with the same identity and operation.
    let again = reserve(&fixture.request(), binding, id).unwrap();
    assert_eq!(again.intent.operation_id, id);
    assert_eq!(status(&fixture.request(), id).unwrap().intent.state, State::Submitted);
}

#[test]
fn a_reservation_for_another_cloud_or_checkout_writes_nothing() {
    let (fixture, binding) = fresh("reserved");
    reserve(&fixture.request(), binding.clone(), OperationId::generate()).unwrap();
    let journal = || std::fs::read(fixture.root.path().join("source/companions.json")).unwrap();
    let before = journal();
    let (_, other) = fresh("elsewhere");
    let id = OperationId::generate();
    let refused = reserve(&fixture.request(), other, id).unwrap_err();
    assert!(refused.to_string().contains("another cloud"), "{refused}");
    let moved = Binding::new(
        &fixture.owner,
        "consumer",
        binding.target().clone(),
        fixture.root.path().join("elsewhere"),
        Origin::Reserved,
    )
    .unwrap();
    let refused = reserve(&fixture.request(), moved, id).unwrap_err();
    assert!(refused.to_string().contains("another checkout"), "{refused}");
    assert_eq!(journal(), before);
    assert!(status(&fixture.request(), id).is_err());
    assert!(!fixture.root.path().join("elsewhere/companion-operation.json").exists());
}

#[test]
fn a_reused_operation_id_is_refused_without_binding_the_companion() {
    let fixture = Fixture::new();
    // An operation of retired ownership: its ID is kept forever, and the alias is unbound.
    let id = fixture.submit(Action::Stop).intent.operation_id;
    execute_with(&fixture.request(), id, &mut Fake::new(&fixture)).unwrap();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.clear();
    assert!(state.intents.retire(&state.owner).unwrap());
    store.save(&state).unwrap();
    drop(store);
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
    let refused = reserve(&fixture.request(), binding, id).unwrap_err();
    assert!(refused.to_string().contains("retired"), "{refused}");
    assert_eq!(bound_checkout(&fixture.request()).unwrap(), None);
    assert!(!fixture.root.path().join("reserved/companion-operation.json").exists());
}

#[test]
fn a_retry_restores_the_claim_of_a_reservation_whose_claim_write_was_lost() {
    let (mut fixture, binding) = fresh("reserved");
    let id = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), id).unwrap();
    // Horizon stopped between the journal write and the target claim.
    let claim = fixture.root.path().join("reserved/companion-operation.json");
    std::fs::remove_file(&claim).unwrap();
    reserve(&fixture.request(), binding, id).unwrap();
    let restored = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
    assert_eq!((restored.id, restored.phase), (id, Phase::Submitted));
    // So the owner's confirmation can still go through once the card exists.
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
}

#[test]
fn an_uncheck_after_execution_started_never_blocks_reconciling_it() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    // The start began, then Horizon stopped before the outcome was recorded. As in a
    // real start, the binding already counts as an existing cloud.
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.intents.mark_existing("reserved");
    state.intents.transition(id, State::Executing).unwrap();
    // The owner unchecked the companion meanwhile.
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    let mut backend = Fake::new(&fixture);
    let reconciled = execute_with(&fixture.request(), id, &mut backend);
    if let Err(error) = &reconciled {
        assert!(!error.to_string().contains("no longer selected"), "{error}");
    }
    let claim = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
    assert_eq!(claim.confirmed, Some(id));
}

#[test]
fn only_an_owner_selection_counts_as_a_selected_cloud() {
    let fixture = Fixture::unbound();
    assert!(selects_a_cloud(&fixture.request()).unwrap());
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    assert!(!selects_a_cloud(&fixture.request()).unwrap());
}

#[test]
fn a_reserved_card_is_recoverable_only_before_any_worker_or_provider_resource() {
    // An existing companion's card is never recreated from its binding.
    assert!(!card_recoverable(&Fixture::new().request()).unwrap());
    let (fixture, _) = reserved();
    // No record yet, then the prepared record a removed card left behind.
    assert!(card_recoverable(&fixture.request()).unwrap());
    save_reserved(&fixture, &prepared(&fixture));
    assert!(card_recoverable(&fixture.request()).unwrap());
    let mut started = prepared(&fixture);
    started.worker = fixture.ready().worker;
    save_reserved(&fixture, &started);
    assert!(!card_recoverable(&fixture.request()).unwrap());
    save_reserved(&fixture, &prepared(&fixture));
    std::fs::write(fixture.root.path().join("reserved/hetzner.json"), "{}").unwrap();
    assert!(!card_recoverable(&fixture.request()).unwrap());
}

#[test]
fn a_created_companion_needs_the_owners_selection_like_any_other_once_created() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), id, &mut backend).unwrap().phase,
        Phase::Ready
    );
    // While still selected, later requests are authorized as before.
    status(&fixture.request(), id).unwrap();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    // After the owner unchecks it, its reserved binding no longer authorizes anything.
    for action in [Action::EnsureReady, Action::Stop] {
        let refused = submit(&fixture.request(), action, OperationId::generate()).unwrap_err();
        assert!(refused.to_string().contains("no longer selected"), "{refused}");
    }
}

#[test]
fn a_recovered_card_for_a_started_operation_continues_without_confirming_again() {
    let (mut fixture, id) = reserved();
    save_reserved(&fixture, &prepared(&fixture));
    select_reserved(&mut fixture);
    confirm_creation(&fixture.request(), id).unwrap();
    // Horizon stopped after the start began, with the binding already marked existing
    // as a real start does; the owner has since unchecked it.
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.intents.mark_existing("reserved");
    state.intents.transition(id, State::Executing).unwrap();
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    // The card's Retry confirms again; that changes nothing, so execution reconciles it.
    let claim = || std::fs::read(fixture.root.path().join("reserved/companion-operation.json")).unwrap();
    let before = claim();
    confirm_creation(&fixture.request(), id).unwrap();
    assert_eq!(claim(), before);
    assert_eq!(status(&fixture.request(), id).unwrap().intent.state, State::Executing);
    assert_eq!(
        submit(&fixture.request(), Action::EnsureReady, id)
            .unwrap()
            .intent
            .operation_id,
        id
    );
    // An operation that was never confirmed is not treated as started under one.
    let (fixture, other) = reserved();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.intents.transition(other, State::Executing).unwrap();
    store.save(&state).unwrap();
    drop(store);
    assert!(confirm_creation(&fixture.request(), other).is_err());
}

#[test]
fn two_aliases_with_the_same_declaration_never_both_reserve_a_cloud() {
    let (mut fixture, binding) = fresh("reserved");
    let declaration = fixture.context.declarations["consumer"].clone();
    fixture.context.declarations.insert("second".into(), declaration);
    reserve(&fixture.request(), binding, OperationId::generate()).unwrap();
    let mut target = fixture.binding.target().clone();
    target.cloud_id = "reserved-second".into();
    let second = Binding::new(
        &fixture.owner,
        "second",
        target,
        fixture.binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    let request = Request {
        alias: "second",
        ..fixture.request()
    };
    let refused = reserve(&request, second, OperationId::generate()).unwrap_err();
    assert!(refused.to_string().contains("being created"), "{refused}");
    assert!(
        !fixture
            .root
            .path()
            .join("reserved-second/companion-operation.json")
            .exists()
    );
}

#[test]
fn two_source_journals_cannot_reserve_the_same_declaration_from_stale_inventory() {
    let (fixture, binding) = fresh("reserved");
    let mut other_owner = fixture.owner.clone();
    other_owner.cloud_id = "other-source".into();
    let mut other_context = fixture.context.clone();
    other_context.source.cloud_id.clone_from(&other_owner.cloud_id);
    let mut target = binding.target().clone();
    target.cloud_id = "second-reserved".into();
    let other_binding = Binding::new(
        &other_owner,
        "consumer",
        target,
        binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    let other = Request {
        owner: &other_owner,
        context: &other_context,
        ..fixture.request()
    };
    let guard = reservation::lock(fixture.root.path()).unwrap();
    assert!(matches!(
        reserve(&other, other_binding.clone(), OperationId::generate()),
        Err(Error::Busy)
    ));
    drop(guard);
    reserve(&fixture.request(), binding, OperationId::generate()).unwrap();
    let error = reserve(&other, other_binding.clone(), OperationId::generate()).unwrap_err();
    assert!(error.to_string().contains("being created"), "{error}");
    assert!(
        !fixture
            .root
            .path()
            .join("second-reserved/companion-operation.json")
            .exists()
    );
    // Reservations in another workspace are independent.
    other_owner.scope.workspace_id = "other-workspace".into();
    other_context.source.scope.clone_from(&other_owner.scope);
    let mut target = other_binding.target().clone();
    target.scope.clone_from(&other_owner.scope);
    let other_binding = Binding::new(
        &other_owner,
        "consumer",
        target,
        other_binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    reserve(
        &Request {
            owner: &other_owner,
            context: &other_context,
            ..fixture.request()
        },
        other_binding,
        OperationId::generate(),
    )
    .unwrap();
}

#[test]
fn declining_an_unallocated_reservation_allows_a_fresh_request_to_reuse_its_identity() {
    for prepared_card in [false, true] {
        let (fixture, binding) = fresh("reserved");
        let id = OperationId::generate();
        reserve(&fixture.request(), binding.clone(), id).unwrap();
        if prepared_card {
            save_reserved(&fixture, &prepared(&fixture));
        }
        cancel_submission(&fixture.request(), id).unwrap();
        let declined = reserve(&fixture.request(), binding.clone(), id).unwrap();
        assert_eq!(declined.phase, Phase::Refused);
        let fresh_id = OperationId::generate();
        let operation = reserve(&fixture.request(), binding, fresh_id).unwrap();
        assert_eq!(operation.intent.target_cloud_id, "reserved");
        assert_eq!(operation.intent.operation_id, fresh_id);
        assert_eq!(operation.phase, Phase::Submitted);
        let mut backend = Fake::new(&fixture);
        assert_eq!(
            execute_with(&fixture.request(), fresh_id, &mut backend).unwrap().phase,
            Phase::ConfirmationRequired
        );
        assert!(backend.decisions.is_empty());
        assert!(confirm_creation(&fixture.request(), fresh_id).is_err());
    }
}

#[test]
fn an_unchecked_existing_unstarted_companion_can_be_declined() {
    let fixture = Fixture::new();
    let mut prepared = fixture.ready();
    prepared.operation = CreateState::Prepared;
    prepared.worker = None;
    fixture.save(&prepared);
    let id = fixture.submit(Action::EnsureReady).intent.operation_id;
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    let mut state = store.load().unwrap();
    state.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&state).unwrap();
    drop(store);
    cancel_submission(&fixture.request(), id).unwrap();
    let (_, state) = fixture.request().load().unwrap();
    assert_eq!(state.intents.operation(id).unwrap().state, State::Failed);
    assert_eq!(
        receipt::load(&fixture.root.path().join("target"))
            .unwrap()
            .unwrap()
            .phase,
        Phase::Refused
    );
}

#[test]
fn reusing_a_declined_identity_cannot_race_a_second_sources_reservation() {
    let (fixture, binding) = fresh("reserved");
    let id = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), id).unwrap();
    cancel_submission(&fixture.request(), id).unwrap();
    let mut owner = fixture.owner.clone();
    owner.cloud_id = "second-source".into();
    let mut context = fixture.context.clone();
    context.source.cloud_id.clone_from(&owner.cloud_id);
    let mut target = binding.target().clone();
    target.cloud_id = "second-reserved".into();
    let second = Binding::new(&owner, "consumer", target, binding.checkout().into(), Origin::Reserved).unwrap();
    let second_request = Request {
        owner: &owner,
        context: &context,
        ..fixture.request()
    };
    let second_id = OperationId::generate();
    reserve(&second_request, second, second_id).unwrap();
    let error = submit(&fixture.request(), Action::EnsureReady, OperationId::generate()).unwrap_err();
    assert!(error.to_string().contains("being created"), "{error}");
    // A real start marks the binding Existing while the operation remains pending.
    let (store, mut state) = second_request.load().unwrap();
    state.intents.mark_existing("second-reserved");
    state.intents.transition(second_id, State::Executing).unwrap();
    store.save(&state).unwrap();
    drop(store);
    assert!(submit(&fixture.request(), Action::EnsureReady, OperationId::generate()).is_err());
    // A settled deployment also fences a caller whose inventory predates its card.
    let (store, mut state) = second_request.load().unwrap();
    state.intents.transition(second_id, State::Succeeded).unwrap();
    store.save(&state).unwrap();
    drop(store);
    let mut deployed = fixture.ready();
    deployed.cloud_id = "second-reserved".into();
    Store::lock(&fixture.root.path().join("second-reserved"))
        .unwrap()
        .save(&deployed)
        .unwrap();
    assert!(submit(&fixture.request(), Action::EnsureReady, OperationId::generate()).is_err());
}

#[test]
fn retry_repairs_a_reused_reservations_claim_when_its_source_write_landed_alone() {
    let (fixture, binding) = fresh("reserved");
    let declined = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), declined).unwrap();
    cancel_submission(&fixture.request(), declined).unwrap();
    let fresh_id = OperationId::generate();
    let (store, mut state) = fixture.request().load().unwrap();
    state.intents.submit("consumer", Action::EnsureReady, fresh_id).unwrap();
    store.save(&state).unwrap();
    drop(store);
    let operation = reserve(&fixture.request(), binding, fresh_id).unwrap();
    assert_eq!(operation.phase, Phase::Submitted);
    let claim = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
    assert_eq!(claim.id, fresh_id);
    assert_eq!(claim.confirmed, None);
}

#[test]
fn submitting_again_repairs_an_interrupted_reuse_with_the_canonical_claim() {
    for (missing_claim, new_retry_id) in [(false, false), (false, true), (true, false), (true, true)] {
        let (mut fixture, binding) = fresh("reserved");
        let declined = OperationId::generate();
        reserve(&fixture.request(), binding, declined).unwrap();
        save_reserved(&fixture, &prepared(&fixture));
        cancel_submission(&fixture.request(), declined).unwrap();
        // The fresh submission committed, then stopped before updating the target.
        let canonical = OperationId::generate();
        let (store, mut state) = fixture.request().load().unwrap();
        state
            .intents
            .submit("consumer", Action::EnsureReady, canonical)
            .unwrap();
        store.save(&state).unwrap();
        drop(store);
        if missing_claim {
            std::fs::remove_file(fixture.root.path().join("reserved/companion-operation.json")).unwrap();
        }
        let retry = if new_retry_id {
            OperationId::generate()
        } else {
            canonical
        };
        let operation = submit(&fixture.request(), Action::EnsureReady, retry).unwrap();
        assert_eq!(
            (operation.intent.operation_id, operation.phase),
            (canonical, Phase::Submitted)
        );
        let claim = receipt::load(&fixture.root.path().join("reserved")).unwrap().unwrap();
        assert_eq!(
            (claim.id, claim.phase, claim.confirmed),
            (canonical, Phase::Submitted, None)
        );
        // The retained card can now confirm and execute that same operation.
        select_reserved(&mut fixture);
        confirm_creation(&fixture.request(), canonical).unwrap();
        let mut backend = Fake::new(&fixture);
        assert_eq!(
            execute_with(&fixture.request(), canonical, &mut backend).unwrap().phase,
            Phase::Ready
        );
        assert_eq!(backend.decisions, [Decision::Reconnect]);
    }
}

#[test]
fn an_existing_binding_with_no_operation_or_cloud_does_not_hold_a_reservation() {
    let (mut fixture, binding) = fresh("reserved");
    bind(&fixture.request(), fixture.binding.clone()).unwrap();
    std::fs::remove_file(fixture.root.path().join("target/deployment.json")).unwrap();
    fixture.owner.cloud_id = "other-source".into();
    fixture.context.source.cloud_id.clone_from(&fixture.owner.cloud_id);
    let binding = Binding::new(
        &fixture.owner,
        "consumer",
        binding.target().clone(),
        binding.checkout().into(),
        Origin::Reserved,
    )
    .unwrap();
    reserve(&fixture.request(), binding, OperationId::generate()).unwrap();
}

#[test]
fn a_reserved_target_started_by_its_card_cannot_resume_without_selection() {
    let (fixture, id) = reserved();
    let mut deployed = fixture.ready();
    deployed.cloud_id = "reserved".into();
    save_reserved(&fixture, &deployed);
    let mut backend = Fake::new(&fixture);
    assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
    assert!(backend.decisions.is_empty());
    let repeated = submit(&fixture.request(), Action::EnsureReady, OperationId::generate()).unwrap();
    assert_eq!(repeated.intent.operation_id, id);
    assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
}

#[test]
fn another_sources_binding_to_the_same_cloud_does_not_block_creation_retry() {
    let (fixture, binding) = fresh("reserved");
    let id = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), id).unwrap();
    save_reserved(&fixture, &prepared(&fixture));
    let mut other = fixture.owner.clone();
    other.cloud_id = "other-source".into();
    let shared = Binding::new(
        &other,
        "consumer",
        binding.target().clone(),
        binding.checkout().into(),
        Origin::Existing,
    )
    .unwrap();
    let store = journal::Store::open(fixture.root.path(), &other).unwrap();
    let mut state = store.load().unwrap();
    state.intents.bind(&other, "consumer", shared).unwrap();
    store.save(&state).unwrap();
    drop(store);
    assert_eq!(
        reserve(&fixture.request(), binding.clone(), id).unwrap().phase,
        Phase::Submitted
    );
    cancel_submission(&fixture.request(), id).unwrap();
    assert_eq!(
        reserve(&fixture.request(), binding, OperationId::generate())
            .unwrap()
            .phase,
        Phase::Submitted
    );
}

#[test]
fn repairing_a_retry_id_restores_its_canonical_claim() {
    let (fixture, binding) = fresh("reserved");
    let canonical = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), canonical).unwrap();
    let retry = OperationId::generate();
    assert_eq!(
        reserve(&fixture.request(), binding.clone(), retry)
            .unwrap()
            .intent
            .operation_id,
        canonical
    );
    std::fs::remove_file(fixture.root.path().join("reserved/companion-operation.json")).unwrap();
    reserve(&fixture.request(), binding, retry).unwrap();
    assert_eq!(
        receipt::load(&fixture.root.path().join("reserved"))
            .unwrap()
            .unwrap()
            .id,
        canonical
    );
}

#[test]
fn a_decline_can_finish_its_claim_write_and_be_retried_after_entering_history() {
    let (fixture, binding) = fresh("reserved");
    let id = OperationId::generate();
    reserve(&fixture.request(), binding.clone(), id).unwrap();
    let (store, mut state) = fixture.request().load().unwrap();
    state.intents.transition(id, State::Failed).unwrap();
    store.save(&state).unwrap();
    drop(store);
    cancel_submission(&fixture.request(), id).unwrap();
    assert_eq!(status(&fixture.request(), id).unwrap().phase, Phase::Refused);
    let new_id = OperationId::generate();
    reserve(&fixture.request(), binding, new_id).unwrap();
    cancel_submission(&fixture.request(), id).unwrap();
    assert_eq!(
        receipt::load(&fixture.root.path().join("reserved"))
            .unwrap()
            .unwrap()
            .id,
        new_id
    );
    assert_eq!(status(&fixture.request(), id).unwrap().phase, Phase::Refused);
}
