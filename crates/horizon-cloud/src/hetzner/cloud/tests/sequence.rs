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
        // A volume a server holds may hold a workspace, so it is never left marked empty.
        assert!(
            !matches!(operation, CreateState::Bound { .. })
                || !self.journal.as_ref().is_some_and(|journal| journal.unused),
            "the volume is marked used before the server is recorded as bound"
        );
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
    // A fresh request first checks that no volume of the operation exists.
    vec![
        (200, listing("volumes", &json!([]))),
        (200, server_types(cores)),
        (200, locations),
        (200, pricing),
    ]
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
    assert_eq!(outcome.served, 4, "only the volume listing and the catalog were read");
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

/// Records whose journal save fails on call `fail_at`, counted from one.
struct Flaky {
    saves: usize,
    fail_at: usize,
    kept: Kept,
}

impl Records for Flaky {
    fn journal(&mut self, journal: &Journal) -> Result<(), CloudError> {
        self.saves += 1;
        if self.saves == self.fail_at {
            return Err(CloudError::Persistence);
        }
        self.kept.journal(journal)
    }

    fn operation(&mut self, operation: &CreateState) -> Result<(), CloudError> {
        self.kept.operation(operation)
    }
}

/// A fresh run whose journal save `fail_at` fails: the caller's journal afterwards
/// and the requests served.
fn failing_save(fail_at: usize, responses: Vec<(u16, Value)>) -> (Journal, usize) {
    let expected = responses.len();
    let (client, requests, task) = provider(responses);
    let policy = Policy {
        locations: vec!["hel1".into()],
        server_types: vec!["cx33".into()],
    };
    let spec = spec();
    let request = Request {
        spec: &spec,
        policy: &policy,
        login: None,
        fresh: true,
    };
    let (mut operation, mut journal) = (CreateState::Prepared, Journal::default());
    let mut records = Flaky {
        saves: 0,
        fail_at,
        kept: Kept::default(),
    };
    let result = provision(
        &client,
        request,
        &mut operation,
        &mut journal,
        &mut records,
        &Cancellation::default(),
        |_| {},
    );
    assert!(matches!(result, Err(CloudError::Persistence)));
    task.join().unwrap();
    let served = requests.lock().unwrap().len();
    assert_eq!(served, expected);
    (journal, served)
}

#[test]
fn a_failed_key_save_leaves_the_callers_journal_as_saved() {
    let (journal, _) = failing_save(1, catalog());
    // A retry with this journal saves the key again before registering it.
    assert_eq!(journal, Journal::default());
}

#[test]
fn a_failed_volume_fence_save_sends_no_volume_request() {
    // Saves: the key, the location, then the volume fence, which fails.
    let (journal, _) = failing_save(3, until_volume());
    assert_eq!(journal.volume, CreateState::Prepared);
    assert!(journal.key.is_some() && journal.location.as_deref() == Some("hel1"));
}

#[test]
fn an_uncertain_volume_request_moves_the_callers_fence_too() {
    let responses = and(until_volume(), [error(503, "unavailable")]);
    let expected = responses.len();
    let (client, requests, task) = provider(responses);
    let policy = Policy {
        locations: vec!["hel1".into()],
        server_types: vec!["cx33".into()],
    };
    let spec = spec();
    let request = Request {
        spec: &spec,
        policy: &policy,
        login: None,
        fresh: true,
    };
    let (mut operation, mut journal) = (CreateState::Prepared, Journal::default());
    let result = provision(
        &client,
        request,
        &mut operation,
        &mut journal,
        &mut Kept::default(),
        &Cancellation::default(),
        |_| {},
    );
    assert!(result.is_err());
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), expected);
    // Saved as requested, so a retry with this journal reconciles instead of
    // sending a second billed volume request.
    assert_eq!(journal.volume, CreateState::Requested);
}

#[test]
fn a_login_for_another_registry_is_refused_before_any_request() {
    let (client, requests, task) = provider(Vec::new());
    let policy = Policy {
        locations: vec!["hel1".into()],
        server_types: vec!["cx33".into()],
    };
    let spec = spec();
    let request = Request {
        spec: &spec,
        policy: &policy,
        login: Some(crate::host::RegistryLogin {
            server: "other.example".into(),
            username: "puller".into(),
            password: crate::Credential::new("secret".into()).unwrap(),
        }),
        fresh: true,
    };
    let (mut operation, mut journal) = (CreateState::Prepared, Journal::default());
    let mut kept = Kept::default();
    let refused = provision(
        &client,
        request,
        &mut operation,
        &mut journal,
        &mut kept,
        &Cancellation::default(),
        |_| {},
    );
    assert!(matches!(refused, Err(CloudError::Invalid(_))));
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty() && kept.journal.is_none());
}

/// Deleting an empty volume: inspect it, delete it, see it gone.
fn released(volume: Value) -> [(u16, Value); 3] {
    let mut inspected = json!({});
    inspected["volume"] = volume;
    [
        (200, inspected),
        (204, Value::Null),
        (404, json!({"error": {"code": "not_found", "message": "not found"}})),
    ]
}

#[test]
fn a_sold_out_only_location_leaves_neither_server_nor_volume() {
    let outcome = fresh(and(
        and(until_server(), [error(412, "resource_unavailable")]),
        released(free()),
    ));
    assert!(outcome.worker.is_none());
    assert_eq!(outcome.operation, CreateState::Prepared, "no server was created");
    assert_eq!(
        outcome.journal.volume,
        CreateState::Prepared,
        "the empty volume was deleted"
    );
    assert_eq!(outcome.journal.location, None);
}

#[test]
fn a_sold_out_location_moves_a_new_cloud_to_the_next_allowed_one() {
    let located = |id: u32, location: &str| {
        let mut value = free();
        value["id"] = json!(id);
        value["location"]["name"] = json!(location);
        value["linux_device"] = json!(format!("/dev/disk/by-id/scsi-0HC_Volume_{id}"));
        value
    };
    let price =
        |location: &str| json!({"location": location, "price_hourly": {"net": "0.01"}, "price_monthly": {"net": "5"}});
    let standing =
        |location: &str| json!({"name": location, "available": true, "recommended": true, "deprecation": null});
    let kind = json!({"name": "cx33", "cores": 4, "memory": 8.0, "disk": 80, "cpu_type": "shared", "architecture": "x86",
        "prices": [price("hel1"), price("nbg1")], "locations": [standing("hel1"), standing("nbg1")]});
    let catalog = vec![
        (200, listing("volumes", &json!([]))),
        (200, listing("server_types", &json!([kind]))),
        (
            200,
            listing(
                "locations",
                &json!([{"name": "hel1", "network_zone": "eu-central"}, {"name": "nbg1", "network_zone": "eu-central"}]),
            ),
        ),
        (
            200,
            json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.05"}}, "primary_ips": []}}),
        ),
    ];
    let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
    let attempt = |volume: Value| {
        vec![
            (200, listing("volumes", &json!([]))),
            (
                201,
                json!({"volume": volume.clone(), "action": {"id": 1, "status": "success"}}),
            ),
            (200, json!({ "volume": volume.clone() })),
            (200, listing("servers", &json!([]))),
            (200, json!({ "volume": volume })),
        ]
    };
    let mut created = created_server(4);
    created["server"]["location"]["name"] = json!("nbg1");
    created["server"]["volumes"] = json!([10]);
    let mut held = located(10, "nbg1");
    held["server"] = json!(42);
    let responses = and(
        and(
            and(
                and(and(catalog, key), attempt(located(9, "hel1"))),
                [error(412, "resource_unavailable")],
            ),
            released(located(9, "hel1")),
        ),
        and(
            attempt(located(10, "nbg1")),
            [(201, created), (200, json!({ "volume": held }))],
        ),
    );
    let (client, requests, task) = provider(responses);
    let policy = Policy {
        locations: vec!["hel1".into(), "nbg1".into()],
        server_types: vec!["cx33".into()],
    };
    let spec = spec();
    let request = Request {
        spec: &spec,
        policy: &policy,
        login: None,
        fresh: true,
    };
    let (mut operation, mut journal) = (CreateState::Prepared, Journal::default());
    let worker = provision(
        &client,
        request,
        &mut operation,
        &mut journal,
        &mut Kept::default(),
        &Cancellation::default(),
        |_| {},
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(worker.data_center(), Some("nbg1"));
    assert_eq!(journal.location.as_deref(), Some("nbg1"));
    assert_eq!(journal.volume, CreateState::Bound { worker_id: "10".into() });
    assert!(!journal.unused, "a server holds the volume now");
    let requests = requests.lock().unwrap();
    assert!(requests.iter().any(|request| request.starts_with("DELETE /volumes/9 ")));
}

#[test]
fn an_empty_volume_an_interrupted_attempt_left_is_deleted_before_the_cloud_is_placed_again() {
    // The earlier attempt created the cloud's first volume, then stopped before any
    // server held it: the location answered sold out, or the process ended.
    let journal = Journal {
        location: Some("hel1".into()),
        volume: CreateState::Bound { worker_id: "9".into() },
        key: Some("ssh-ed25519 AAAA".into()),
        unused: true,
        ..Journal::default()
    };
    let outcome = run(
        CreateState::Prepared,
        journal,
        false,
        and(released(free()).to_vec(), catalog_of(2)),
    );
    assert!(!outcome.journal.unused, "the cleanup finished");
    assert_eq!(outcome.journal.volume, CreateState::Prepared);
    assert_eq!(outcome.journal.location, None);
}

#[test]
fn a_volume_found_by_label_is_adopted_and_never_deleted_when_sold_out() {
    // No volume recorded, but one exists: it may hold work, so it is never deleted.
    let found = [(200, listing("volumes", &json!([free()])))];
    let rest: Vec<(u16, Value)> = catalog().into_iter().skip(1).collect();
    let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
    let adopted = [
        (200, listing("volumes", &json!([free()]))),
        (200, json!({"volume": free()})),
    ];
    let responses = and(
        and(and(and(found.to_vec(), rest), key), adopted),
        vec![
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free()})),
            error(412, "resource_unavailable"),
        ],
    );
    let outcome = run(CreateState::Prepared, Journal::default(), false, responses);
    assert!(outcome.worker.is_none());
    assert_eq!(
        outcome.journal.volume,
        CreateState::Bound { worker_id: "9".into() },
        "the found volume stays"
    );
}

#[test]
fn a_resumed_cloud_whose_volume_held_a_workspace_is_never_moved_when_sold_out() {
    // A stop released the server that held this volume; resuming finds its location sold out.
    let journal = Journal {
        location: Some("hel1".into()),
        volume: CreateState::Bound { worker_id: "9".into() },
        key: Some(super::super::throwaway_public_key().unwrap()),
        ..Journal::default()
    };
    let rest: Vec<(u16, Value)> = catalog().into_iter().skip(1).collect();
    let responses = and(
        and(rest, [(200, listing("ssh_keys", &json!([]))), (201, key())]),
        vec![
            (200, json!({"volume": free()})),
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free()})),
            error(412, "resource_unavailable"),
        ],
    );
    let outcome = run(CreateState::Prepared, journal, false, responses);
    assert!(outcome.worker.is_none());
    assert_eq!(outcome.served, 9, "the sold-out answer ends it, with no delete");
    assert_eq!(
        outcome.journal.volume,
        CreateState::Bound { worker_id: "9".into() },
        "the workspace stays"
    );
    assert_eq!(outcome.journal.location.as_deref(), Some("hel1"));
}

#[test]
fn a_volume_reconciled_after_a_lost_answer_is_kept_where_it_is_when_sold_out() {
    // The volume request went out and its answer was lost: the volume found now may
    // be that one or an older one, so it is never deleted to move the cloud.
    let journal = Journal {
        location: Some("hel1".into()),
        volume: CreateState::Requested,
        key: Some(super::super::throwaway_public_key().unwrap()),
        ..Journal::default()
    };
    let rest: Vec<(u16, Value)> = catalog().into_iter().skip(1).collect();
    let responses = and(
        and(rest, [(200, listing("ssh_keys", &json!([]))), (201, key())]),
        vec![
            (200, listing("volumes", &json!([free()]))),
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free()})),
            error(412, "resource_unavailable"),
        ],
    );
    let outcome = run(CreateState::Prepared, journal, false, responses);
    assert!(outcome.worker.is_none());
    assert_eq!(outcome.served, 9, "reconciled and refused, with no delete");
    assert_eq!(outcome.journal.volume, CreateState::Bound { worker_id: "9".into() });
    assert!(!outcome.journal.unused);
}

#[test]
fn a_volume_whose_name_is_taken_is_adopted_as_used_and_never_deleted_when_sold_out() {
    // Both listings missed a volume that exists, so the create finds its name taken.
    let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
    let responses = and(
        and(catalog(), key),
        vec![
            (200, listing("volumes", &json!([]))),
            error(409, "uniqueness_error"),
            (200, listing("volumes", &json!([free()]))),
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free()})),
            error(412, "resource_unavailable"),
        ],
    );
    let outcome = run(CreateState::Prepared, Journal::default(), false, responses);
    assert!(outcome.worker.is_none());
    assert_eq!(outcome.served, 12, "sold out, with no delete");
    assert_eq!(outcome.journal.volume, CreateState::Bound { worker_id: "9".into() });
    assert!(!outcome.journal.unused);
}

#[test]
fn a_volume_a_lagging_listing_hid_is_adopted_as_used_and_never_deleted_when_sold_out() {
    // The first listing misses a volume that exists; the one before creating finds it.
    let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
    let adopted = [(200, listing("volumes", &json!([free()])))];
    let responses = and(
        and(and(catalog(), key), adopted),
        vec![
            (200, listing("servers", &json!([]))),
            (200, json!({"volume": free()})),
            error(412, "resource_unavailable"),
        ],
    );
    let outcome = run(CreateState::Prepared, Journal::default(), false, responses);
    assert!(outcome.worker.is_none());
    assert_eq!(outcome.served, 10, "sold out, with no delete");
    assert_eq!(outcome.journal.volume, CreateState::Bound { worker_id: "9".into() });
    assert!(!outcome.journal.unused, "an adopted volume may hold a workspace");
}

#[test]
fn the_pull_login_is_checked_before_a_server_is_created_and_never_on_a_reconnect() {
    let policy = Policy {
        locations: vec!["hel1".into()],
        server_types: vec!["cx33".into()],
    };
    let spec = spec();
    let registry = spec.image_digest.split('/').next().unwrap().to_owned();
    let login = || crate::host::RegistryLogin {
        server: registry.clone(),
        username: "puller".into(),
        password: crate::Credential::new("secret".into()).unwrap(),
    };
    let request = |login| Request {
        spec: &spec,
        policy: &policy,
        login: Some(login),
        fresh: true,
    };
    // A new server: the refused login stops everything before the first request.
    let (client, requests, task) = provider(Vec::new());
    let client = client.with_pull_check(|_, _, _| Err(CloudError::Invalid("refused by the registry")));
    let (mut operation, mut journal, mut kept) = (CreateState::Prepared, Journal::default(), Kept::default());
    let refused = provision(
        &client,
        request(login()),
        &mut operation,
        &mut journal,
        &mut kept,
        &Cancellation::default(),
        |_| {},
    );
    task.join().unwrap();
    assert!(matches!(refused, Err(CloudError::Invalid("refused by the registry"))));
    assert!(requests.lock().unwrap().is_empty() && kept.journal.is_none());
    // A requested server is only reconciled; its login is never checked again.
    let (client, requests, task) = provider(vec![(200, listing("servers", &json!([])))]);
    let client = client.with_pull_check(|_, _, _| panic!("a reconnect checks no login"));
    let mut operation = CreateState::Requested;
    let mut journal = Journal {
        location: Some("hel1".into()),
        volume: CreateState::Bound { worker_id: "9".into() },
        key: Some(super::super::throwaway_public_key().unwrap()),
        ..Journal::default()
    };
    let _ = provision(
        &client,
        request(login()),
        &mut operation,
        &mut journal,
        &mut Kept::default(),
        &Cancellation::default(),
        |_| {},
    );
    task.join().unwrap();
    assert!(
        !requests.lock().unwrap().is_empty(),
        "the reconnect went on to the provider"
    );
}
