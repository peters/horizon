use super::*;
use crate::cloud_runtime::companions::{
    self, Catalog,
    transport::{Transport, Worker},
};
use horizon_cloud_protocol::companion::{Request, Response};

struct Offline;
impl Transport for Offline {
    fn worker(&mut self, _: &str) -> Result<Option<Worker>> {
        Ok(None)
    }
    fn release(&mut self, _: &str) {}
    fn publish(&mut self, _: &str, _: &Catalog) -> Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &str, _: &Request) -> Result<Response> {
        panic!("migration without grants never performs remote changes")
    }
}

fn moved_owner() -> Owner {
    let mut moved = owner();
    moved.scope.workspace_id = "new-workspace".into();
    moved
}

#[test]
fn settled_owner_moves_archive_history_and_require_new_selection() {
    let temp = tempfile::tempdir().unwrap();
    let store = companions::journal::Store::open(temp.path(), &owner()).unwrap();
    let mut state = store.load().unwrap();
    state.intents = journal();
    let first = state
        .intents
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    let retry = OperationId::generate();
    state.intents.submit("consumer", Action::EnsureReady, retry).unwrap();
    state.intents.transition(first.operation_id, State::Failed).unwrap();
    store.save(&state).unwrap();
    drop(store);
    let moved = moved_owner();
    let store = companions::journal::Store::open(temp.path(), &moved).unwrap();
    companions::execute(&store, &moved, None, &companions::Action::Refresh, &mut Offline).unwrap();
    let mut restored = store.load().unwrap();
    assert_eq!(restored.owner, moved);
    assert!(restored.intents.binding("consumer").is_none());
    assert!(restored.intents.operation(first.operation_id).is_none());
    assert_eq!(restored.intents.retired.len(), 1);
    assert_eq!(restored.intents.retired[0].owner, owner());
    assert_eq!(
        restored.intents.retired[0]
            .journal
            .operation(retry)
            .unwrap()
            .operation_id,
        first.operation_id
    );
    let mut target = binding("consumer", Origin::Reserved).target;
    target.scope = moved.scope.clone();
    let new_binding = Binding::new(&moved, "consumer", target, std::env::temp_dir(), Origin::Reserved).unwrap();
    restored.intents.bind(&moved, "consumer", new_binding).unwrap();
    for id in [first.operation_id, retry] {
        assert!(restored.intents.submit("consumer", Action::EnsureReady, id).is_err());
    }
    store.save(&restored).unwrap();
    companions::execute(&store, &owner(), None, &companions::Action::Refresh, &mut Offline).unwrap();
    let mut returned = store.load().unwrap();
    assert_eq!(returned.owner, owner());
    assert!(returned.intents.binding("consumer").is_none());
    returned
        .intents
        .bind(&owner(), "consumer", binding("consumer", Origin::Reserved))
        .unwrap();
    assert!(returned.intents.submit("consumer", Action::EnsureReady, retry).is_err());
    returned
        .intents
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    store.save(&returned).unwrap();
}

#[test]
fn pending_operations_keep_their_owner_until_definitively_settled() {
    for pending in [State::Submitted, State::Executing, State::Uncertain] {
        let temp = tempfile::tempdir().unwrap();
        let store = companions::journal::Store::open(temp.path(), &owner()).unwrap();
        let mut state = store.load().unwrap();
        state.intents = journal();
        let intent = state
            .intents
            .submit("consumer", Action::EnsureReady, OperationId::generate())
            .unwrap();
        if pending != State::Submitted {
            state.intents.transition(intent.operation_id, State::Executing).unwrap();
        }
        if pending == State::Uncertain {
            state.intents.transition(intent.operation_id, pending).unwrap();
        }
        store.save(&state).unwrap();
        let snapshot =
            companions::execute(&store, &moved_owner(), None, &companions::Action::Refresh, &mut Offline).unwrap();
        let mut restored = store.load().unwrap();
        assert_eq!(restored.owner, owner());
        assert!(restored.intents.retired.is_empty());
        assert!(snapshot.notice.unwrap().contains("pending companion operations"));
        restored.intents.transition(intent.operation_id, State::Failed).unwrap();
        store.save(&restored).unwrap();
        companions::execute(&store, &moved_owner(), None, &companions::Action::Refresh, &mut Offline).unwrap();
        assert_eq!(store.load().unwrap().owner, moved_owner());
    }
}

#[test]
fn oversized_archive_never_replaces_the_durable_journal() {
    let temp = tempfile::tempdir().unwrap();
    let store = companions::journal::Store::open(temp.path(), &owner()).unwrap();
    let mut state = store.load().unwrap();
    store.save(&state).unwrap();
    state.intents = journal();
    state.intents.bindings.get_mut("consumer").unwrap().checkout = std::env::temp_dir().join("a".repeat(256 * 1024));
    assert!(store.save(&state).is_err());
    assert!(store.load().unwrap().intents.is_empty());
}

#[test]
fn archive_only_moves_keep_fences_without_growing_history() {
    let mut journal = journal();
    let first = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    journal.transition(first.operation_id, State::Failed).unwrap();
    assert!(journal.retire(&owner()).unwrap());
    for current in [moved_owner(), owner(), moved_owner()] {
        assert!(journal.retire(&current).unwrap());
        journal.validate(&current).unwrap();
        assert_eq!(journal.retired.len(), 1);
        assert!(journal.active_empty());
        assert!(!journal.is_empty());
        assert!(
            journal
                .submit("consumer", Action::EnsureReady, first.operation_id)
                .is_err()
        );
    }
}

#[test]
fn current_and_retired_operations_cannot_share_ids_or_retain_pending_work() {
    let mut journal = journal();
    let first = journal
        .submit("consumer", Action::EnsureReady, OperationId::generate())
        .unwrap();
    journal.transition(first.operation_id, State::Failed).unwrap();
    journal.retire(&owner()).unwrap();
    journal
        .bind(&owner(), "consumer", binding("consumer", Origin::Reserved))
        .unwrap();
    let current = journal
        .submit("consumer", Action::Stop, OperationId::generate())
        .unwrap();
    journal.intents.get_mut("consumer").unwrap().operation_id = first.operation_id;
    assert!(journal.validate(&owner()).is_err());
    journal.intents.get_mut("consumer").unwrap().operation_id = current.operation_id;
    journal.retired[0].journal.intents.get_mut("consumer").unwrap().state = State::Uncertain;
    assert!(journal.validate(&owner()).is_err());
}
