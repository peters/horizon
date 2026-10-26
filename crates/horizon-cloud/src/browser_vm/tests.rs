//! The rental sequence of a browser VM, against a provider that answers with a script:
//! the state volume is named after the machine and mounted at `/state`, a stop keeps it,
//! a resume attaches it again, a delete removes it, and a failed create leaves only
//! records that a delete finishes.
use super::*;
use crate::{CreateState, hetzner::tests::provider};
use serde_json::{Value, json};

/// A real Ed25519 public key, as Cloud settings makes one.
const PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";
/// The server and volume the scripted provider reports.
const SERVER: u64 = 42;
const VOLUME: u64 = 9;
const ADDRESS: &str = "203.0.113.10";

fn request() -> Request {
    Request {
        image: format!("ghcr.io/example/browser-vm@sha256:{}", "a".repeat(64)),
        server_type: "ccx13".into(),
        location: "fsn1".into(),
        volume_gb: 20,
        public_key: PUBLIC_KEY.into(),
    }
}

fn listing(key: &str, items: Value) -> Value {
    let mut page = json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items;
    page
}

fn error(status: u16) -> (u16, Value) {
    (
        status,
        json!({"error": {"code": "unavailable", "message": "unavailable"}}),
    )
}

/// The volume of the machine, as Hetzner reports it, at `server`.
fn volume_at(server: Option<u64>) -> Value {
    json!({
        "id": VOLUME,
        "name": crate::hetzner::resource_name(OPERATION_ID).unwrap(),
        "size": 20,
        "location": {"name": "fsn1"},
        "server": server,
        "linux_device": format!("/dev/disk/by-id/scsi-0HC_Volume_{VOLUME}"),
        "status": "available",
        "labels": {"horizon-operation": OPERATION_ID},
    })
}

fn server(status: &str) -> Value {
    json!({
        "id": SERVER,
        "name": crate::hetzner::resource_name(OPERATION_ID).unwrap(),
        "status": status,
        "public_net": {"ipv4": {"ip": ADDRESS}},
        "server_type": {"name": "ccx13", "cores": 2, "memory": 8.0, "disk": 80,
            "prices": [{"location": "fsn1", "price_hourly": {"net": "0.0200"}, "price_monthly": {"net": "12.49"}}]},
        "location": {"name": "fsn1", "network_zone": "eu-central"},
        "labels": {"horizon-operation": OPERATION_ID},
        "volumes": [VOLUME],
        "private_net": [{"network": 7, "ip": "10.72.0.2", "alias_ips": [], "mac_address": "86:00:00:00:00:01"}],
    })
}

/// A key as Hetzner reports it. A create answers with the key the request sent, which
/// the scripted provider echoes; a listing must name it instead.
fn key(public_key: &str) -> Value {
    json!({"ssh_key": {"id": 5, "name": crate::hetzner::resource_name(OPERATION_ID).unwrap(),
        "public_key": public_key, "labels": {"horizon-operation": OPERATION_ID}}})
}

fn created_key() -> Value {
    key("@PUBLIC_KEY@")
}

/// The reads that start any new request: the volume listing and the server catalog.
fn catalog() -> Vec<(u16, Value)> {
    let price = json!({"location": "fsn1", "price_hourly": {"net": "0.0200"}, "price_monthly": {"net": "12.49"}});
    let server_type = json!({"name": "ccx13", "cores": 2, "memory": 8.0, "disk": 80, "cpu_type": "dedicated",
        "architecture": "x86", "prices": [price],
        "locations": [{"name": "fsn1", "available": true, "recommended": true, "deprecation": null}]});
    let locations = listing("locations", json!([{"name": "fsn1", "network_zone": "eu-central"}]));
    let pricing =
        json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.0440"}}, "primary_ips": []}});
    vec![
        (200, listing("server_types", json!([server_type]))),
        (200, locations),
        (200, pricing),
    ]
}

/// The key and the volume request up to the server request.
fn until_server() -> Vec<(u16, Value)> {
    // A fresh request first checks that no volume of the machine exists.
    let mut responses = vec![(200, listing("volumes", json!([])))];
    responses.extend(catalog());
    // The key that opens the machine is registered before anything is billed.
    responses.extend([(200, listing("ssh_keys", json!([]))), (201, created_key())]);
    // The volume is requested in the location the settings name, and attached by the
    // server request instead of through its own action.
    responses.extend([
        (200, listing("volumes", json!([]))),
        (
            201,
            json!({"volume": volume_at(None), "action": {"id": 1, "status": "success"}}),
        ),
        (200, json!({"volume": volume_at(None)})),
        (
            200,
            listing("locations", json!([{"name": "fsn1", "network_zone": "eu-central"}])),
        ),
        (
            200,
            listing(
                "networks",
                json!([{"id": 7, "name": "horizon-eu-central", "ip_range": "10.72.0.0/16",
                    "subnets": [{"type": "cloud", "ip_range": "10.72.0.0/17", "network_zone": "eu-central"}],
                    "labels": {"horizon-network": "eu-central"}}]),
            ),
        ),
        (200, listing("servers", json!([]))),
        (200, json!({"volume": volume_at(None)})),
    ]);
    responses
}

fn created_server() -> Value {
    json!({"server": server("running"), "action": {"id": 2, "status": "success"}, "next_actions": []})
}

/// The records a caller keeps, in memory.
#[derive(Default)]
struct Kept {
    journal: Option<Journal>,
    operation: Option<CreateState>,
    stops: Vec<crate::hetzner::cloud::Stop>,
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
    fn stop(&mut self, stop: crate::hetzner::cloud::Stop) -> Result<(), CloudError> {
        self.stops.push(stop);
        Ok(())
    }
}

/// What a run left in the caller's records.
struct Recorded {
    operation: CreateState,
    journal: Journal,
    requests: Vec<String>,
}

fn recorded(mut kept: Kept, operation: CreateState, journal: Journal, requests: Vec<String>) -> Recorded {
    Recorded {
        operation: kept.operation.take().unwrap_or(operation),
        journal: kept.journal.take().unwrap_or(journal),
        requests,
    }
}

/// Deploys `request` from the given records against `responses`.
fn deploy(
    request: &Request,
    operation: CreateState,
    journal: Journal,
    responses: Vec<(u16, Value)>,
) -> (Result<Machine, CloudError>, Recorded) {
    let (client, requests, task) = provider(responses);
    let (mut operation_fence, mut kept_journal) = (operation.clone(), journal.clone());
    let mut kept = Kept::default();
    let deployed = deploy_with(&client, request, &mut operation_fence, &mut kept_journal, &mut kept);
    task.join().unwrap();
    let served = requests.lock().unwrap().clone();
    (deployed, recorded(kept, operation, journal, served))
}

fn deploy_with(
    client: &crate::hetzner::Hetzner,
    request: &Request,
    operation: &mut CreateState,
    journal: &mut Journal,
    records: &mut Kept,
) -> Result<Machine, CloudError> {
    super::deploy(
        client,
        request,
        None,
        Fences { operation, journal },
        records,
        &Cancellation::default(),
        |_| {},
    )
}

fn body(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap_or(Value::Null)
}

#[test]
fn a_browser_vm_is_rented_with_a_named_state_volume_at_state() {
    let mut responses = until_server();
    responses.push((201, created_server()));
    // Settling reads the volume again, which now holds the server.
    responses.push((200, json!({"volume": volume_at(Some(SERVER))})));
    let (deployed, recorded) = deploy(&request(), CreateState::Prepared, Journal::default(), responses);
    let machine = deployed.unwrap();

    // The volume is named after the machine and created in the requested location.
    let created = recorded
        .requests
        .iter()
        .find(|request| request.starts_with("POST /volumes "))
        .map(|request| body(request))
        .expect("the state volume is requested");
    assert_eq!(created["name"], crate::hetzner::resource_name(OPERATION_ID).unwrap());
    assert_eq!(created["location"], "fsn1");
    assert_eq!(created["size"], 20);

    // The server attaches it without automount, and its user data mounts it at /state.
    let created = recorded
        .requests
        .iter()
        .find(|request| request.starts_with("POST /servers "))
        .map(|request| body(request))
        .expect("the server is requested");
    assert_eq!(created["volumes"], json!([VOLUME]));
    assert_eq!(created["automount"], Value::Bool(false));
    assert_eq!(created["server_type"], "ccx13");
    let user_data = created["user_data"].as_str().unwrap();
    assert!(
        user_data.contains("- - /dev/disk/by-id/scsi-0HC_Volume_9\n  - /state"),
        "the state volume is mounted where the image keeps its profiles: {user_data}"
    );
    assert!(user_data.contains("-p 127.0.0.1:5900:5900"), "{user_data}");

    // The machine is the browser host the settings name, reached with the person's key.
    assert_eq!(machine.ssh_target, format!("root@{ADDRESS}"));
    assert_eq!(machine.server_id, SERVER.to_string());
    assert_eq!(machine.volume_id, VOLUME.to_string());
    assert_eq!(
        (machine.location.as_str(), machine.server_type.as_str()),
        ("fsn1", "ccx13")
    );
    assert_eq!(machine.hourly_net, Some(0.02));
    assert_eq!(
        recorded.operation,
        CreateState::Bound {
            worker_id: SERVER.to_string()
        }
    );
}

#[test]
fn a_tag_instead_of_a_digest_is_refused_before_anything_is_rented() {
    let mut tagged = request();
    tagged.image = "ghcr.io/example/browser-vm:1780".into();
    let (deployed, recorded) = deploy(&tagged, CreateState::Prepared, Journal::default(), Vec::new());
    let error = deployed.unwrap_err().to_string();
    assert!(error.contains("digest"), "{error}");
    assert!(recorded.requests.is_empty(), "no request reaches the provider");
}

#[test]
fn a_failed_server_request_leaves_only_what_a_delete_removes() {
    // The server request is refused with a reason that does not settle it, so the
    // volume stays recorded and the machine is reachable for a delete.
    let mut responses = until_server();
    responses.push(error(503));
    let (deployed, recorded) = deploy(&request(), CreateState::Prepared, Journal::default(), responses);
    assert!(deployed.is_err());
    assert_eq!(recorded.operation, CreateState::Requested);
    assert_eq!(
        recorded.journal.volume,
        CreateState::Bound {
            worker_id: VOLUME.to_string()
        }
    );
    assert!(
        recorded.journal.retains(),
        "the volume may exist, so it is not forgotten"
    );

    // The delete finds no server, removes the volume and the key, and proves both gone.
    let (client, requests, task) = provider(vec![
        // No server carries the label, so the uncertain request created none.
        (200, listing("servers", json!([]))),
        (200, listing("servers", json!([]))),
        (200, json!({"volume": volume_at(None)})),
        (204, Value::Null),
        (404, json!({"error": {"code": "not_found", "message": "not found"}})),
        // The key is found, deleted, and proven gone.
        (200, listing("ssh_keys", json!([key(PUBLIC_KEY)["ssh_key"].clone()]))),
        (200, listing("ssh_keys", json!([key(PUBLIC_KEY)["ssh_key"].clone()]))),
        (204, Value::Null),
        (200, listing("ssh_keys", json!([]))),
    ]);
    let (mut operation, mut journal) = (recorded.operation, recorded.journal);
    let mut kept = Kept::default();
    let deleted = super::delete(
        &client,
        &mut Fences {
            operation: &mut operation,
            journal: &mut journal,
        },
        &mut kept,
        &Cancellation::default(),
    );
    task.join().unwrap();
    let served = requests.lock().unwrap();
    deleted.unwrap();
    assert!(served.iter().any(|request| request.starts_with("DELETE /volumes/9 ")));
    assert!(served.iter().any(|request| request.starts_with("DELETE /ssh_keys/5 ")));
    assert!(!kept.journal.unwrap().retains(), "nothing of the machine remains");
}

#[test]
fn a_rental_from_the_fences_of_a_finished_delete_is_refused_until_they_are_cleared() {
    // A delete proves the server and the volume gone but keeps their fences and the delete
    // intent, so a rental from those records is refused. The record of the machine clears
    // them when a deploy follows a finished delete, and the rental then starts from the
    // clean fences that `a_browser_vm_is_rented_with_a_named_state_volume_at_state` uses.
    let deleted = Journal {
        location: Some("fsn1".into()),
        volume: CreateState::Terminated {
            worker_id: VOLUME.to_string(),
        },
        deleting: true,
        ..Journal::default()
    };
    let (deployed, recorded) = deploy(
        &request(),
        CreateState::Terminated {
            worker_id: SERVER.to_string(),
        },
        deleted,
        Vec::new(),
    );
    let error = deployed.unwrap_err().to_string();
    assert!(error.contains("Finish deleting"), "{error}");
    assert!(recorded.requests.is_empty(), "nothing reaches the provider");
}

#[test]
fn a_stop_releases_the_server_and_keeps_the_state_volume() {
    let bound = CreateState::Bound {
        worker_id: SERVER.to_string(),
    };
    let journal = Journal {
        location: Some("fsn1".into()),
        volume: CreateState::Bound {
            worker_id: VOLUME.to_string(),
        },
        key: Some(PUBLIC_KEY.into()),
        ..Journal::default()
    };
    let (client, requests, task) = provider(vec![
        // The stop reads the server, then finds it already off, then deletes it.
        (200, json!({"server": server("off")})),
        (200, json!({"server": server("off")})),
        (200, json!({"server": server("off")})),
        (200, listing("actions", json!([]))),
        (200, json!({"action": {"id": 3, "status": "success"}})),
        (404, json!({"error": {"code": "not_found", "message": "not found"}})),
    ]);
    let (mut released, mut kept_journal) = (bound.clone(), journal);
    let mut kept = Kept::default();
    super::stop(
        &client,
        &mut Fences {
            operation: &mut released,
            journal: &mut kept_journal,
        },
        &mut kept,
        &Cancellation::default(),
    )
    .unwrap();
    task.join().unwrap();
    let served = requests.lock().unwrap();
    assert!(served.iter().any(|request| request.starts_with("DELETE /servers/42 ")));
    assert!(
        !served.iter().any(|request| request.starts_with("DELETE /volumes/")),
        "the sign-ins stay"
    );
    let saved = kept.journal.unwrap();
    assert_eq!(
        saved.volume,
        CreateState::Bound {
            worker_id: VOLUME.to_string()
        }
    );
    assert_eq!(saved.released.as_deref(), Some(SERVER.to_string().as_str()));
    assert!(saved.retains());
    assert_eq!(
        kept.operation.unwrap_or(bound.clone()),
        bound,
        "the fence keeps the released server"
    );
}

#[test]
fn a_resume_rents_a_new_server_on_the_same_state_volume() {
    let stopped = CreateState::Bound {
        worker_id: SERVER.to_string(),
    };
    let journal = Journal {
        location: Some("fsn1".into()),
        volume: CreateState::Bound {
            worker_id: VOLUME.to_string(),
        },
        key: Some(PUBLIC_KEY.into()),
        released: Some(SERVER.to_string()),
        ..Journal::default()
    };
    let (mut operation, mut resuming) = (stopped, journal);
    super::prepare_resume(&mut operation, &mut resuming, true).unwrap();
    assert_eq!(operation, CreateState::Prepared);
    assert!(resuming.released.is_none() && !resuming.unused);

    // A resume places a new server on the volume that is already there.
    // A stopped machine already has its volume, so the reads start at the catalog.
    let mut responses = catalog();
    // The key and the volume already exist, so both are found by label.
    responses.extend([
        (200, listing("ssh_keys", json!([key(PUBLIC_KEY)["ssh_key"].clone()]))),
        (200, json!({"volume": volume_at(None)})),
        (
            200,
            listing("locations", json!([{"name": "fsn1", "network_zone": "eu-central"}])),
        ),
        (
            200,
            listing(
                "networks",
                json!([{"id": 7, "name": "horizon-eu-central", "ip_range": "10.72.0.0/16",
                    "subnets": [{"type": "cloud", "ip_range": "10.72.0.0/17", "network_zone": "eu-central"}],
                    "labels": {"horizon-network": "eu-central"}}]),
            ),
        ),
        (200, listing("servers", json!([]))),
        (200, json!({"volume": volume_at(None)})),
        (201, created_server()),
        (200, json!({"volume": volume_at(Some(SERVER))})),
    ]);
    let (deployed, recorded) = deploy(&request(), operation, resuming, responses);
    let machine = deployed.unwrap();
    assert_eq!(machine.volume_id, VOLUME.to_string());
    assert_eq!(machine.ssh_target, format!("root@{ADDRESS}"));
    assert!(
        !recorded
            .requests
            .iter()
            .any(|request| request.starts_with("POST /volumes ")),
        "a resume never creates a second state volume"
    );
    let created = recorded
        .requests
        .iter()
        .find(|request| request.starts_with("POST /servers "))
        .map(|request| body(request))
        .expect("a new server is requested");
    assert_eq!(created["volumes"], json!([VOLUME]));
}
