//! Deleting every record a provisioning failure leaves, and checking a server a
//! stop did not release.
use super::{server, spec, volume};
use crate::{
    Cancellation, CloudError, CreateState,
    hetzner::{
        cloud::{Cloud, Journal, Policy, Records, Stop, StopRecords, check, delete},
        tests::provider,
    },
};
use serde_json::{Value, json};
use std::time::Duration;

/// The records a caller would persist, kept in memory.
#[derive(Default)]
struct Kept {
    journal: Option<Journal>,
    operation: Option<CreateState>,
    stops: Vec<Stop>,
}

impl Records for Kept {
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError> {
        self.journal = Some(journal.clone());
        Ok(())
    }

    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError> {
        self.operation = Some(operation.clone());
        Ok(())
    }
}

impl StopRecords for Kept {
    fn stop(&mut self, stop: Stop) -> Result<(), CloudError> {
        self.stops.push(stop);
        Ok(())
    }
}

fn listing(key: &str, items: &Value) -> Value {
    let mut page = json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items.clone();
    page
}
fn gone() -> (u16, Value) {
    (404, json!({"error": {"code": "not_found", "message": "not found"}}))
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
fn journal(volume: CreateState) -> Journal {
    Journal {
        location: Some("hel1".into()),
        volume,
        key: Some("ssh-ed25519 AAAA".into()),
        ..Journal::default()
    }
}

/// Deletes a cloud whose records are `operation` and `journal` against
/// `responses`; returns the server fence as last recorded and whether the
/// journal still retains anything.
fn delete_from(operation: CreateState, mut journal: Journal, responses: Vec<(u16, Value)>) -> (CreateState, bool) {
    let (client, requests, task) = provider(responses);
    let mut operation = operation;
    let mut kept = Kept::default();
    let (operation_id, cancel) = (spec().operation_id, Cancellation::default());
    let cloud = Cloud {
        client: &client,
        operation_id: &operation_id,
        cancel: &cancel,
    };
    delete(cloud, &mut operation, &mut journal, &mut kept, Duration::ZERO).unwrap();
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with("DELETE "))
    );
    let recorded = kept.journal.unwrap();
    assert!(recorded.deleting, "the delete intent stays until the cloud is reopened");
    (kept.operation.unwrap_or(operation), recorded.retains())
}

/// The key listings of a delete that finds and removes the cloud's key.
fn key_deleted() -> [(u16, Value); 4] {
    [
        (200, listing("ssh_keys", &json!([key()]))),
        (200, listing("ssh_keys", &json!([key()]))),
        (204, Value::Null),
        (200, listing("ssh_keys", &json!([]))),
    ]
}

#[test]
fn a_registered_key_alone_is_deleted() {
    let only_key = Journal {
        key: Some("ssh-ed25519 AAAA".into()),
        ..Journal::default()
    };
    let (_, kept) = delete_from(CreateState::Prepared, only_key, key_deleted().to_vec());
    assert!(!kept, "nothing remains, so the cloud can be removed");
}

#[test]
fn an_uncertain_volume_is_found_by_label_and_deleted() {
    let mut responses = vec![
        (200, listing("volumes", &json!([free_volume()]))),
        (200, json!({"volume": free_volume()})),
        (204, Value::Null),
        gone(),
    ];
    responses.extend(key_deleted());
    let (_, kept) = delete_from(CreateState::Prepared, journal(CreateState::Requested), responses);
    assert!(!kept);
}

#[test]
fn an_uncertain_server_is_found_by_label_and_deleted_with_its_volume() {
    let server = serde_json::to_value(server("running", Some("192.0.2.10"))).unwrap();
    let (operation, kept) = delete_from(
        CreateState::Requested,
        journal(CreateState::Bound { worker_id: "9".into() }),
        vec![
            (200, listing("servers", &json!([server]))),
            (200, json!({"server": server})),
            (200, json!({"action": {"id": 3, "status": "success"}})),
            gone(),
            (200, json!({"volume": free_volume()})),
            (204, Value::Null),
            gone(),
            (200, listing("ssh_keys", &json!([]))),
            (200, listing("ssh_keys", &json!([]))),
        ],
    );
    assert_eq!(operation, CreateState::Terminated { worker_id: "42".into() });
    assert!(!kept);
}

#[test]
fn a_volume_request_that_created_nothing_is_settled_after_a_second_look() {
    let mut responses = vec![
        (200, listing("volumes", &json!([]))),
        (200, listing("volumes", &json!([]))),
    ];
    responses.extend(key_deleted());
    let (_, kept) = delete_from(CreateState::Prepared, journal(CreateState::Requested), responses);
    assert!(!kept);
}

#[test]
fn a_server_request_that_created_nothing_is_settled_and_its_volume_deleted() {
    let mut responses = vec![
        (200, listing("servers", &json!([]))),
        (200, listing("servers", &json!([]))),
        (200, json!({"volume": free_volume()})),
        (204, Value::Null),
        gone(),
    ];
    responses.extend(key_deleted());
    let (operation, kept) = delete_from(
        CreateState::Requested,
        journal(CreateState::Bound { worker_id: "9".into() }),
        responses,
    );
    assert_eq!(operation, CreateState::Prepared);
    assert!(!kept);
}

#[test]
fn a_powered_off_server_that_was_not_released_is_never_reported_stopped() {
    let off = (200, json!({"server": server("off", None)}));
    let held = (200, json!({"volume": volume()}));
    let (client, _, task) = provider(vec![off, held]);
    let mut operation = CreateState::Bound { worker_id: "42".into() };
    let policy = || {
        Ok(Policy {
            locations: vec!["hel1".into()],
            server_types: vec!["cx33".into()],
        })
    };
    let mut kept = Kept::default();
    let (operation_id, cancel) = (spec().operation_id, Cancellation::default());
    let cloud = Cloud {
        client: &client,
        operation_id: &operation_id,
        cancel: &cancel,
    };
    let volume = journal(CreateState::Bound { worker_id: "9".into() });
    let refused = check(cloud, Some(&spec()), &mut operation, &volume, &policy, &mut kept).unwrap_err();
    task.join().unwrap();
    assert!(refused.to_string().contains("still billed"));
    assert!(kept.stops.is_empty() && kept.operation.is_none(), "nothing is recorded");
}
