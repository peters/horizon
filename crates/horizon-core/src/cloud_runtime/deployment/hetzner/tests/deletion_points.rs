//! Deletion cleans up the record each provisioning failure point leaves, and an
//! interrupted stop is finished rather than taken for a released server.
use super::{provider, server, spec, volume};
use crate::cloud_runtime::{
    Stage,
    deployment::hetzner::{
        Allowed, Compute, Journal,
        lifecycle::{delete_with, reconcile_with, stop_with},
        retained,
    },
    state::{Deployment, Store},
};
use horizon_cloud::{Cancellation, CreateState, Credential, hetzner::Hetzner};
use serde_json::{Value, json};

fn listing(key: &str, items: &Value) -> String {
    let mut page = json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items.clone();
    page.to_string()
}
fn gone() -> (u16, String) {
    (
        404,
        json!({"error": {"code": "not_found", "message": "not found"}}).to_string(),
    )
}
fn key() -> Value {
    json!({"id": 5, "name": format!("horizon-cloud-{}", spec().operation_id),
        "public_key": "ssh-ed25519 AAAA", "labels": {"horizon-operation": spec().operation_id}})
}
fn free_volume() -> Value {
    let mut value = serde_json::to_value(volume()).unwrap();
    value["server"] = Value::Null;
    value
}

/// Deletes a cloud whose record is `operation` and `journal`, against `responses`.
fn delete_from(operation: &CreateState, journal: &Journal, responses: Vec<(u16, String)>) -> (Deployment, bool) {
    let (state, kept, requests) = act_on(operation, journal, responses, |compute, store, state| {
        delete_with(compute, store, state, &Cancellation::default()).unwrap();
    });
    assert!(requests.iter().any(|request| request.starts_with("DELETE ")));
    (state, kept)
}

/// Runs `act` on a cloud whose record is `operation` and `journal`, stopping,
/// against `responses`; returns the saved deployment, whether anything is
/// retained and the requests served.
fn act_on(
    operation: &CreateState,
    journal: &Journal,
    responses: Vec<(u16, String)>,
    act: impl FnOnce(&Compute, &Store, &mut Deployment),
) -> (Deployment, bool, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let spec = spec();
    let mut state: Deployment = serde_json::from_value(json!({
        "version": 1, "cloud_id": spec.operation_id, "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec.profile, "stage": "Stopping", "operation": operation, "spec": spec,
        "worker": null, "sessions": [], "stop_requested": true
    }))
    .unwrap();
    store.save(&state).unwrap();
    journal.save(root.path()).unwrap();
    let (address, requests, task) = provider::serve(responses);
    let compute = Compute {
        client: Hetzner::loopback(Credential::new("secret-test-key".into()).unwrap(), address).unwrap(),
        settings: serde_json::from_value(
            json!({"token_file": "/unused", "server_types": ["cx33"], "locations": ["hel1"]}),
        )
        .unwrap(),
        allowed: Allowed {
            locations: vec!["hel1".into()],
            server_types: vec!["cx33".into()],
        },
        registries: None,
    };
    act(&compute, &store, &mut state);
    task.join().unwrap();
    let requests = requests.lock().unwrap().clone();
    (store.load().unwrap().unwrap(), retained(root.path()).unwrap(), requests)
}

fn journal(volume: CreateState) -> Journal {
    Journal {
        location: Some("hel1".into()),
        volume,
        key: Some("ssh-ed25519 AAAA".into()),
        released: None,
        deleting: false,
    }
}

#[test]
fn a_registered_key_alone_is_deleted() {
    let mut only_key = journal(CreateState::Prepared);
    only_key.location = None;
    let (_, kept) = delete_from(
        &CreateState::Prepared,
        &only_key,
        vec![
            (200, listing("ssh_keys", &json!([key()]))),
            (200, listing("ssh_keys", &json!([key()]))),
            (204, String::new()),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert!(!kept, "nothing remains, so the cloud can be removed");
}

#[test]
fn an_uncertain_volume_is_found_by_label_and_deleted() {
    let (_, kept) = delete_from(
        &CreateState::Prepared,
        &journal(CreateState::Requested),
        vec![
            (200, listing("volumes", &json!([free_volume()]))),
            (200, json!({"volume": free_volume()}).to_string()),
            (204, String::new()),
            gone(),
            (200, listing("ssh_keys", &json!([key()]))),
            (200, listing("ssh_keys", &json!([key()]))),
            (204, String::new()),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert!(!kept);
}

#[test]
fn an_uncertain_server_is_found_by_label_and_deleted_with_its_volume() {
    let server = serde_json::to_value(server("running", Some("192.0.2.10"))).unwrap();
    let (state, kept) = delete_from(
        &CreateState::Requested,
        &journal(CreateState::Bound { worker_id: "9".into() }),
        vec![
            (200, listing("servers", &json!([server]))),
            (200, json!({"server": server}).to_string()),
            (200, json!({"action": {"id": 3, "status": "success"}}).to_string()),
            gone(),
            (200, json!({"volume": free_volume()}).to_string()),
            (204, String::new()),
            gone(),
            (200, listing("ssh_keys", &json!([]))),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert_eq!(state.operation, CreateState::Terminated { worker_id: "42".into() });
    assert!(!kept);
}

#[test]
fn an_interrupted_stop_is_neither_reported_stopped_nor_resumed_until_its_server_is_gone() {
    let server = serde_json::to_value(server("off", None)).unwrap();
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut released = journal(CreateState::Bound { worker_id: "9".into() });
    released.released = Some("42".into());
    let still_there = (200, json!({"server": server}).to_string());
    let (state, _, requests) = act_on(&bound, &released, vec![still_there.clone()], |compute, store, state| {
        // A stop that crashed after recording only its release.
        (state.stage, state.stop_requested) = (Stage::Ready, false);
        let report = reconcile_with(compute, store, state, &Cancellation::default()).unwrap();
        assert!(report.worker.is_none(), "no stopped worker while the server exists");
    });
    assert_eq!(requests.len(), 1);
    assert!(
        state.stop_requested && state.stage == Stage::Stopping,
        "the stop intent is restored"
    );
    // Stopping again deletes the server the first stop released, after checking it is off.
    let deleted = vec![
        still_there.clone(),
        still_there,
        (200, json!({"action": {"id": 3, "status": "success"}}).to_string()),
        gone(),
    ];
    let (state, kept, requests) = act_on(&bound, &released, deleted, |compute, store, state| {
        stop_with(compute, store, state, &Cancellation::default()).unwrap();
    });
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with("DELETE ") && request.contains("/servers/42 "))
    );
    assert_eq!((state.stage, state.operation), (Stage::Stopped, bound));
    assert!(kept, "the workspace volume stays");
}

#[test]
fn a_stop_records_the_release_before_it_shuts_the_server_down() {
    let running = (
        200,
        json!({"server": server("running", Some("192.0.2.10"))}).to_string(),
    );
    let refused = (
        503,
        json!({"error": {"code": "unavailable", "message": "unavailable"}}).to_string(),
    );
    let bound = CreateState::Bound { worker_id: "42".into() };
    let volume = journal(CreateState::Bound { worker_id: "9".into() });
    let responses = vec![running.clone(), running.clone(), running, refused];
    let (state, kept, _) = act_on(&bound, &volume, responses, |compute, store, state| {
        assert!(stop_with(compute, store, state, &Cancellation::default()).is_err());
        assert_eq!(Journal::load(store.root()).unwrap().released.as_deref(), Some("42"));
    });
    // A failed shutdown leaves an unfinished stop, which stopping again completes.
    assert_eq!((state.stage, state.operation), (Stage::Stopping, bound));
    assert!(kept);
}

#[test]
fn a_volume_request_that_created_nothing_is_settled_after_a_second_look() {
    let (_, kept) = delete_from(
        &CreateState::Prepared,
        &journal(CreateState::Requested),
        vec![
            (200, listing("volumes", &json!([]))),
            (200, listing("volumes", &json!([]))),
            (200, listing("ssh_keys", &json!([key()]))),
            (200, listing("ssh_keys", &json!([key()]))),
            (204, String::new()),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert!(!kept);
}

#[test]
fn a_server_request_that_created_nothing_is_settled_and_its_volume_deleted() {
    let (state, kept) = delete_from(
        &CreateState::Requested,
        &journal(CreateState::Bound { worker_id: "9".into() }),
        vec![
            (200, listing("servers", &json!([]))),
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free_volume()}).to_string()),
            (204, String::new()),
            gone(),
            (200, listing("ssh_keys", &json!([key()]))),
            (200, listing("ssh_keys", &json!([key()]))),
            (204, String::new()),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert_eq!(state.operation, CreateState::Prepared);
    assert!(!kept);
}

#[test]
fn a_powered_off_server_that_was_not_released_is_never_reported_stopped() {
    let off = (200, json!({"server": server("off", None)}).to_string());
    let held = (200, json!({"volume": volume()}).to_string());
    let bound = CreateState::Bound { worker_id: "42".into() };
    let journal = journal(CreateState::Bound { worker_id: "9".into() });
    act_on(&bound, &journal, vec![off, held], |compute, store, state| {
        let refused = reconcile_with(compute, store, state, &Cancellation::default()).unwrap_err();
        assert!(refused.to_string().contains("still billed"));
    });
}

#[test]
fn a_stop_interrupted_after_its_delete_is_finished_by_check_without_a_recorded_worker() {
    let bound = CreateState::Bound { worker_id: "42".into() };
    let mut released = journal(CreateState::Bound { worker_id: "9".into() });
    released.released = Some("42".into());
    let (state, kept, _) = act_on(&bound, &released, vec![gone()], |compute, store, state| {
        assert!(state.worker.is_none());
        reconcile_with(compute, store, state, &Cancellation::default()).unwrap();
    });
    assert!(
        state.stop_requested && state.stage == Stage::Stopped,
        "resume is now possible"
    );
    assert!(kept, "the workspace volume stays");
}
