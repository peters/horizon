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
    let (fixture, id) = reserved();
    cancel_submission(&fixture.request(), id).unwrap();
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
