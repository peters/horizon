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
    let mut backend = Fake::new(&fixture);
    assert!(execute_with(&fixture.request(), id, &mut backend).is_err());
    assert!(backend.decisions.is_empty());
    assert_eq!(status(&fixture.request(), id).unwrap().intent.state, State::Submitted);
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
    receipt::confirm(&target, &fixture.owner, id).unwrap();
    receipt::save(&target, &fixture.owner, id, Phase::Running).unwrap();
    assert_eq!(receipt::load(target.root()).unwrap().unwrap().confirmed, Some(id));
}
