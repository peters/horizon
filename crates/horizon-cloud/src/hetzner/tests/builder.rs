use super::{
    keys::key,
    servers::{posts, server},
    *,
};
use crate::{
    CreateState,
    hetzner::{builder::HostKey, builder::Request, servers::Placement},
};

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

const BUILDER_LOGIN_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

/// A create response whose server has the type and location that were asked for.
fn created_as(id: u64, server_type: &str, location: &str) -> Value {
    let mut server = server(id);
    server["server_type"]["name"] = json!(server_type);
    server["location"]["name"] = json!(location);
    json!({"server": server, "action": action(1, "success"), "next_actions": [], "root_password": null})
}

fn builder_request(placements: &[Placement]) -> Request<'_> {
    Request {
        operation_id: OPERATION,
        placements,
        login_key: BUILDER_LOGIN_KEY,
        host_key: HostKey {
            private: "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n",
            public: "ssh-ed25519 AAAAC3Nza builder\n",
        },
    }
}

#[test]
fn a_type_the_project_refuses_leaves_the_rent_to_the_next_placement() {
    let placements = [
        Placement {
            server_type: "cpx42".into(),
            location: "fsn1".into(),
        },
        Placement {
            server_type: "ccx33".into(),
            location: "fsn1".into(),
        },
    ];
    let (hetzner, requests, task) = provider(vec![
        (200, listing("ssh_keys", json!([]))),
        (201, json!({"ssh_key": key(5)})),
        (200, listing("servers", json!([]))),
        (403, error("resource_limit_exceeded", "shared core limit exceeded")),
        (200, listing("servers", json!([]))),
        (201, created_as(42, "ccx33", "fsn1")),
    ]);
    let mut state = CreateState::Prepared;
    let mut persisted = Vec::new();
    let server = hetzner
        .rent_builder(
            &builder_request(&placements),
            &mut state,
            &Cancellation::default(),
            |state| {
                persisted.push(state.clone());
                Ok(())
            },
        )
        .unwrap();
    task.join().unwrap();
    assert_eq!(server.id, 42);
    let asked: Vec<Value> = posts(&requests)
        .into_iter()
        .map(|body| body["server_type"].clone())
        .collect();
    assert_eq!(
        asked,
        [json!("cpx42"), json!("ccx33")],
        "the refused type is followed by the next placement"
    );
    assert_eq!(
        persisted,
        [
            CreateState::Requested,
            CreateState::Prepared,
            CreateState::Requested,
            CreateState::Bound { worker_id: "42".into() }
        ],
        "each request is journaled, and a refusal leaves no server behind"
    );
}

#[test]
fn every_placement_refused_reports_the_last_refusal_and_keeps_no_server() {
    let placements = [Placement {
        server_type: "cpx42".into(),
        location: "fsn1".into(),
    }];
    let (hetzner, requests, task) = provider(vec![
        (200, listing("ssh_keys", json!([]))),
        (201, json!({"ssh_key": key(5)})),
        (200, listing("servers", json!([]))),
        (403, error("resource_limit_exceeded", "shared core limit exceeded")),
    ]);
    let mut state = CreateState::Prepared;
    let error = hetzner
        .rent_builder(
            &builder_request(&placements),
            &mut state,
            &Cancellation::default(),
            |_| Ok(()),
        )
        .unwrap_err();
    task.join().unwrap();
    assert!(
        matches!(error, CloudError::Rejected(_)),
        "the refusal is told as the provider gave it: {error}"
    );
    assert_eq!(state, CreateState::Prepared, "no request is left for a server");
    let requests = requests.lock().unwrap();
    assert!(!requests.iter().any(|request| request.starts_with("DELETE")));
}
