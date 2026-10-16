use super::{servers::server, *};
use crate::CreateState;

#[test]
fn a_builder_whose_create_had_no_answer_is_found_by_its_label_and_deleted() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([server(42)]))),
        (404, error("not_found", "server not found")),
        (200, listing("ssh_keys", json!([]))),
        (200, listing("ssh_keys", json!([]))),
    ]);
    let mut state = CreateState::Requested;
    let mut persisted = Vec::new();
    hetzner
        .return_builder(OPERATION, &mut state, false, &Cancellation::default(), |state| {
            persisted.push(state.clone());
            Ok(())
        })
        .unwrap();
    task.join().unwrap();
    assert_eq!(
        persisted,
        [
            CreateState::Bound { worker_id: "42".into() },
            CreateState::Terminated { worker_id: "42".into() }
        ],
        "the found server is journaled before it is deleted"
    );
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /servers?label_selector="));
    assert!(requests[1].starts_with("GET /servers/42 "));
    assert!(requests[2].starts_with("GET /ssh_keys?label_selector="));
}

#[test]
fn a_builder_never_created_only_loses_its_key_and_two_servers_fail_closed() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (200, listing("servers", json!([]))),
        (200, listing("ssh_keys", json!([]))),
        (200, listing("ssh_keys", json!([]))),
        (200, listing("servers", json!([server(42), server(43)]))),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Requested;
    assert!(
        matches!(
            hetzner.return_builder(OPERATION, &mut state, false, &cancel, |_| panic!("nothing to journal")),
            Err(CloudError::CreationUnresolved)
        ),
        "a request that may still make a server keeps its journal"
    );
    hetzner
        .return_builder(OPERATION, &mut state, true, &cancel, |_| panic!("nothing to journal"))
        .unwrap();
    assert_eq!(state, CreateState::Requested);
    let mut state = CreateState::Requested;
    assert!(matches!(
        hetzner.return_builder(OPERATION, &mut state, true, &cancel, |_| Ok(())),
        Err(CloudError::DuplicateWorkers)
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(!requests.iter().any(|request| request.starts_with("DELETE")));
}
