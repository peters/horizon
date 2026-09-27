//! Deletion cleans up the record each provisioning failure point leaves.
use super::{provider, server, spec, volume};
use crate::cloud_runtime::{
    deployment::hetzner::{Allowed, Compute, Journal, lifecycle::delete_with, retained},
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
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let spec = spec();
    let mut state: Deployment = serde_json::from_value(json!({
        "version": 1, "cloud_id": spec.operation_id, "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec.profile, "stage": "Provision", "operation": operation, "spec": spec,
        "worker": null, "sessions": []
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
    };
    delete_with(&compute, &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with("DELETE "))
    );
    (store.load().unwrap().unwrap(), retained(root.path()).unwrap())
}

fn journal(volume: CreateState) -> Journal {
    Journal {
        location: Some("hel1".into()),
        volume,
        key: Some("ssh-ed25519 AAAA".into()),
        released: None,
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
