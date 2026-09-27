//! The provisioning sequence at each failure point: what is recorded, and that
//! nothing is created before everything is checked.
use super::{spec, volume};
use crate::{
    Cancellation, CloudError, CreateState, Worker,
    hetzner::{
        cloud::{Journal, Policy, Records, Request, provision},
        tests::provider,
    },
};
use serde_json::{Value, json};

/// The records a caller would persist, kept in memory.
#[derive(Default)]
struct Kept {
    journal: Option<Journal>,
    operation: Option<CreateState>,
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

/// What a run left: the server fence and the journal as last recorded, whether
/// the journal retains anything, the requests served and the worker, if any.
struct Outcome {
    operation: CreateState,
    journal: Journal,
    kept: bool,
    served: usize,
    worker: Option<Worker>,
}

fn listing(key: &str, items: &Value) -> Value {
    let mut page = json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items.clone();
    page
}
fn error(status: u16, code: &str) -> (u16, Value) {
    (status, json!({"error": {"code": code, "message": code}}))
}
fn server_types(cores: u32) -> Value {
    let price = json!({"location": "hel1", "price_hourly": {"net": "0.01"}, "price_monthly": {"net": "5"}});
    let kind = json!({"name": "cx33", "cores": cores, "memory": 8.0, "disk": 80, "cpu_type": "shared",
        "architecture": "x86", "prices": [price],
        "locations": [{"name": "hel1", "available": true, "recommended": true, "deprecation": null}]});
    listing("server_types", &json!([kind]))
}
fn catalog_of(cores: u32) -> Vec<(u16, Value)> {
    let locations = listing("locations", &json!([{"name": "hel1", "network_zone": "eu-central"}]));
    let pricing =
        json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.05"}}, "primary_ips": []}});
    vec![(200, server_types(cores)), (200, locations), (200, pricing)]
}
/// The catalog reads that start every fresh request.
fn catalog() -> Vec<(u16, Value)> {
    catalog_of(4)
}
fn key() -> Value {
    json!({"ssh_key": {"id": 5, "name": format!("horizon-cloud-{}", spec().operation_id), "public_key": "@PUBLIC_KEY@",
        "labels": {"horizon-operation": spec().operation_id}}})
}
fn free() -> Value {
    let mut value = serde_json::to_value(volume()).unwrap();
    value["server"] = Value::Null;
    value
}
fn held_volume() -> Value {
    json!({"volume": serde_json::to_value(volume()).unwrap()})
}
fn and(mut responses: Vec<(u16, Value)>, more: impl IntoIterator<Item = (u16, Value)>) -> Vec<(u16, Value)> {
    responses.extend(more);
    responses
}
/// A fresh request up to its volume request.
fn until_volume() -> Vec<(u16, Value)> {
    let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
    and(and(catalog(), key), [(200, listing("volumes", &json!([])))])
}
/// A fresh request up to its server request: the key, the volume and the checks between.
fn until_server() -> Vec<(u16, Value)> {
    let volume = [
        (201, json!({"volume": free(), "action": {"id": 1, "status": "success"}})),
        (200, json!({"volume": free()})),
    ];
    and(
        and(until_volume(), volume),
        [(200, listing("servers", &json!([]))), (200, json!({"volume": free()}))],
    )
}
fn created_server(cores: u32) -> Value {
    json!({"server": {"id": 42, "name": format!("horizon-cloud-{}", spec().operation_id), "status": "initializing",
        "public_net": {"ipv4": null}, "server_type": {"name": "cx33", "cores": cores, "memory": 8.0, "disk": 80},
        "location": {"name": "hel1"}, "labels": {"horizon-operation": spec().operation_id}, "volumes": [9]},
        "action": {"id": 2, "status": "success"}, "next_actions": []})
}

/// Provisions from `operation`, `journal` and `fresh` against `responses`.
fn run(operation: CreateState, journal: Journal, fresh: bool, responses: Vec<(u16, Value)>) -> Outcome {
    run_spec(&spec(), operation, journal, fresh, responses)
}

fn run_spec(
    spec: &crate::WorkerSpec,
    mut operation: CreateState,
    mut journal: Journal,
    fresh: bool,
    responses: Vec<(u16, Value)>,
) -> Outcome {
    let (client, requests, task) = provider(responses);
    let policy = Policy {
        locations: vec!["hel1".into()],
        server_types: vec!["cx33".into()],
    };
    let request = Request {
        spec,
        policy: &policy,
        login: None,
        fresh,
    };
    // What the caller had recorded before: a run is judged only by what it records.
    let (recorded_operation, recorded_journal) = (operation.clone(), journal.clone());
    let mut kept = Kept::default();
    let worker = provision(
        &client,
        request,
        &mut operation,
        &mut journal,
        &mut kept,
        &Cancellation::default(),
        |_| {},
    )
    .ok();
    task.join().unwrap();
    let journal = kept.journal.unwrap_or(recorded_journal);
    Outcome {
        operation: kept.operation.unwrap_or(recorded_operation),
        kept: journal.retains(),
        journal,
        served: requests.lock().unwrap().len(),
        worker,
    }
}

fn fresh(responses: Vec<(u16, Value)>) -> Outcome {
    run(CreateState::Prepared, Journal::default(), true, responses)
}

#[test]
fn a_volume_size_hetzner_refuses_is_refused_before_any_request() {
    let mut small = spec();
    small.profile.storage.volume_gb = 5;
    let outcome = run_spec(&small, CreateState::Prepared, Journal::default(), true, Vec::new());
    assert_eq!((outcome.served, outcome.kept), (0, false));
}

#[test]
fn a_cloud_whose_stop_is_unfinished_is_never_reconnected() {
    let bound = CreateState::Bound { worker_id: "42".into() };
    let journal = Journal {
        released: Some("42".into()),
        ..Journal::default()
    };
    let outcome = run(bound.clone(), journal, true, Vec::new());
    assert_eq!((outcome.operation, outcome.served), (bound, 0));
}

#[test]
fn a_redeployed_cloud_requests_a_new_volume_after_its_deleted_one() {
    // An unfinished delete, with its key, volume or server left, is refused before any request,
    // and so is a finished one while the caller still holds the old workspace.
    let terminated = CreateState::Terminated { worker_id: "8".into() };
    let key = Some("ssh-ed25519 AAAA".to_owned());
    for (key, volume, operation, fresh) in [
        (key.clone(), terminated.clone(), CreateState::Prepared, true),
        (None, terminated.clone(), CreateState::Requested, true),
        (key, CreateState::Prepared, CreateState::Prepared, true),
        (None, terminated, CreateState::Prepared, false),
    ] {
        let journal = Journal {
            volume,
            key,
            deleting: true,
            ..Journal::default()
        };
        let outcome = run(operation, journal, fresh, Vec::new());
        assert_eq!((outcome.served, outcome.journal.location), (0, None));
    }
    let deleted = Journal {
        location: Some("nbg1".into()),
        volume: CreateState::Terminated { worker_id: "8".into() },
        ..Journal::default()
    };
    let outcome = run(
        CreateState::Prepared,
        deleted,
        true,
        and(until_volume(), [error(503, "unavailable")]),
    );
    assert_eq!(
        (outcome.journal.volume, outcome.journal.location.as_deref()),
        (CreateState::Requested, Some("hel1"))
    );
}

#[test]
fn nothing_is_created_when_no_type_fits() {
    let outcome = fresh(catalog_of(2));
    assert_eq!(outcome.served, 3, "only the catalog was read");
    assert!(outcome.journal.key.is_none() && outcome.journal.location.is_none() && !outcome.kept);
}

#[test]
fn a_failed_key_registration_leaves_the_key_recorded_for_deletion() {
    let outcome = fresh(and(
        catalog(),
        [(200, listing("ssh_keys", &json!([]))), error(503, "unavailable")],
    ));
    // A key may exist on Hetzner, so the cloud keeps it; no volume was
    // requested, so no location is fixed.
    assert!(outcome.journal.key.is_some() && outcome.kept && outcome.journal.location.is_none());
    assert_eq!(
        (outcome.journal.volume, outcome.operation),
        (CreateState::Prepared, CreateState::Prepared)
    );
}

#[test]
fn an_uncertain_volume_request_stays_fenced_for_reconciliation() {
    let outcome = fresh(and(until_volume(), [error(503, "unavailable")]));
    assert_eq!(outcome.journal.volume, CreateState::Requested);
    // The location is recorded before the request it fixes.
    assert!(outcome.kept && outcome.journal.location.as_deref() == Some("hel1"));
    assert_eq!(outcome.operation, CreateState::Prepared);
}

#[test]
fn an_uncertain_server_request_stays_fenced_with_its_volume_bound() {
    let outcome = fresh(and(until_server(), [error(503, "unavailable")]));
    assert_eq!(outcome.journal.volume, CreateState::Bound { worker_id: "9".into() });
    assert_eq!(outcome.operation, CreateState::Requested);
    assert!(outcome.kept && outcome.worker.is_none());
}

#[test]
fn a_server_below_the_profile_is_bound_but_never_a_worker() {
    let outcome = fresh(and(until_server(), [(201, created_server(2)), (200, held_volume())]));
    // Bound so it can be deleted, but never described as the worker.
    assert_eq!(outcome.operation, CreateState::Bound { worker_id: "42".into() });
    assert!(outcome.worker.is_none());
    assert!(outcome.kept && matches!(outcome.journal.volume, CreateState::Bound { .. }));
}

#[test]
fn a_server_whose_volume_does_not_name_it_is_never_a_worker() {
    let outcome = fresh(and(
        until_server(),
        [(201, created_server(4)), (200, json!({"volume": free()}))],
    ));
    assert_eq!(outcome.operation, CreateState::Bound { worker_id: "42".into() });
    assert!(outcome.worker.is_none() && outcome.kept);
}

#[test]
fn a_verified_server_becomes_the_worker() {
    let outcome = fresh(and(until_server(), [(201, created_server(4)), (200, held_volume())]));
    assert_eq!(outcome.worker.unwrap().id, "42");
}
