use super::*;
use crate::hetzner::{
    keys::SshKey,
    servers::{Placement, Server, ServerRequest},
    volumes::Volume,
};
use crate::{CreateState, Progress, WorkerStatus};

pub(super) fn server(id: u64) -> Value {
    json!({"id": id, "name": "horizon-cloud-op-1", "status": "running",
        "public_net": {"ipv4": {"ip": "192.0.2.10"}, "ipv6": null},
        "server_type": {"name": "cpx22", "cores": 2, "memory": 4.0, "disk": 80},
        "location": {"name": "hel1"},
        "labels": {"horizon-operation": OPERATION}, "volumes": []})
}

fn placements() -> Vec<Placement> {
    [("cpx22", "hel1"), ("cpx32", "nbg1")]
        .map(|(server_type, location)| Placement {
            server_type: server_type.into(),
            location: location.into(),
        })
        .to_vec()
}

fn request(placements: &[Placement]) -> ServerRequest<'_> {
    ServerRequest {
        operation_id: OPERATION,
        placements,
        image: "docker-ce",
        user_data: "#cloud-config\n",
        volume: None,
        ssh_key: None,
    }
}

fn created(id: u64) -> Value {
    json!({"server": server(id), "action": action(1, "success"), "next_actions": [], "root_password": null})
}

fn posts(requests: &Requests) -> Vec<Value> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.starts_with("POST /servers "))
        .map(|request| request_body(request))
        .collect()
}

#[test]
fn create_persists_the_fence_before_posting_and_binds_the_server() {
    let placements = placements();
    let (hetzner, requests, task) = provider(vec![(200, listing("servers", json!([]))), (201, created(42))]);
    let mut state = CreateState::Prepared;
    let mut saved = Vec::new();
    let mut reported = Vec::new();
    let found = hetzner
        .ensure_server(
            &request(&placements),
            &mut state,
            &Cancellation::default(),
            |next| {
                saved.push(next.clone());
                Ok(())
            },
            |step| reported.push(step),
        )
        .unwrap();
    task.join().unwrap();
    assert_eq!(found.id, 42);
    assert_eq!(found.status(), WorkerStatus::Running);
    assert_eq!(found.ssh_address().unwrap().to_string(), "192.0.2.10:22");
    let bound = CreateState::Bound { worker_id: "42".into() };
    assert_eq!(saved, [CreateState::Requested, bound.clone()]);
    assert_eq!(state, bound);
    assert_eq!(
        reported,
        [
            Progress::Reconciling,
            Progress::Requesting,
            Progress::WorkerFound("42".into())
        ]
    );
    assert!(requests.lock().unwrap()[0].starts_with("GET /servers?label_selector=horizon-operation%3Dop-1&page=1"));
    let body = &posts(&requests)[0];
    assert_eq!(body["name"], "horizon-cloud-op-1");
    assert_eq!(body["labels"], json!({"horizon-operation": OPERATION}));
    assert_eq!(
        (&body["server_type"], &body["location"]),
        (&json!("cpx22"), &json!("hel1"))
    );
    assert_eq!(body["image"], "docker-ce");
    assert_eq!(body["user_data"], "#cloud-config\n");
    assert!(body.get("volumes").is_none() && body.get("automount").is_none());
}

#[test]
fn a_capacity_refusal_moves_to_the_next_placement_and_exhaustion_leaves_it_prepared() {
    let placements = placements();
    let out_of_stock = || (412, error("resource_unavailable", "server type unavailable"));
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        out_of_stock(),
        (201, created(42)),
        (200, listing("servers", json!([]))),
        out_of_stock(),
        out_of_stock(),
    ]);
    let mut state = CreateState::Prepared;
    let mut saved = Vec::new();
    let cancel = Cancellation::default();
    hetzner
        .ensure_server(
            &request(&placements),
            &mut state,
            &cancel,
            |next| {
                saved.push(next.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    assert_eq!(
        saved,
        [
            CreateState::Requested,
            CreateState::Prepared,
            CreateState::Requested,
            CreateState::Bound { worker_id: "42".into() }
        ]
    );
    let mut state = CreateState::Prepared;
    let refusal = hetzner
        .ensure_server(&request(&placements), &mut state, &cancel, |_| Ok(()), |_| {})
        .unwrap_err();
    task.join().unwrap();
    assert_eq!(
        refusal.to_string(),
        "Provider rejected the request or capacity is unavailable: server type unavailable"
    );
    assert_eq!(state, CreateState::Prepared);
    let tried: Vec<_> = posts(&requests).iter().map(|body| body["location"].clone()).collect();
    assert_eq!(tried, ["hel1", "nbg1", "hel1", "nbg1"]);
}

#[test]
fn an_uncertain_create_is_never_repeated_and_later_reconciles_by_label() {
    let placements = placements();
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (503, error("unavailable", "service unavailable")),
        (200, listing("servers", json!([]))),
        (200, listing("servers", json!([server(42)]))),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Prepared;
    let ensure =
        |state: &mut CreateState| hetzner.ensure_server(&request(&placements), state, &cancel, |_| Ok(()), |_| {});
    assert!(matches!(ensure(&mut state), Err(CloudError::Http(503, _))));
    assert_eq!(state, CreateState::Requested);
    assert!(matches!(ensure(&mut state), Err(CloudError::CreationUnresolved)));
    assert_eq!(state, CreateState::Requested);
    assert_eq!(ensure(&mut state).unwrap().id, 42);
    assert_eq!(state, CreateState::Bound { worker_id: "42".into() });
    task.join().unwrap();
    assert_eq!(posts(&requests).len(), 1);
}

#[test]
fn a_taken_name_reconciles_and_a_definite_refusal_can_retry() {
    let placements = placements();
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (409, error("uniqueness_error", "server name is already used")),
        (200, listing("servers", json!([server(42)]))),
        (200, listing("servers", json!([]))),
        (422, error("invalid_input", "invalid user data")),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Prepared;
    let found = hetzner
        .ensure_server(&request(&placements), &mut state, &cancel, |_| Ok(()), |_| {})
        .unwrap();
    assert_eq!(found.id, 42);
    let mut state = CreateState::Prepared;
    assert!(matches!(
        hetzner.ensure_server(&request(&placements), &mut state, &cancel, |_| Ok(()), |_| {}),
        Err(CloudError::Rejected(_))
    ));
    assert_eq!(state, CreateState::Prepared);
    task.join().unwrap();
    assert_eq!(
        posts(&requests).len(),
        2,
        "a non-capacity refusal does not try the next placement"
    );
}

#[test]
fn existing_servers_are_adopted_only_with_their_identity_and_duplicates_fail_closed() {
    let placements = placements();
    let mut foreign = server(43);
    foreign["name"] = json!("unrelated");
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([server(42)]))),
        (200, listing("servers", json!([server(42), server(43)]))),
        (200, listing("servers", json!([foreign]))),
        (200, json!({"server": server(42)})),
        (404, error("not_found", "server not found")),
    ]);
    let cancel = Cancellation::default();
    let ensure =
        |state: &mut CreateState| hetzner.ensure_server(&request(&placements), state, &cancel, |_| Ok(()), |_| {});
    let mut state = CreateState::Prepared;
    assert_eq!(ensure(&mut state).unwrap().id, 42);
    assert!(matches!(
        ensure(&mut CreateState::Prepared),
        Err(CloudError::DuplicateWorkers)
    ));
    assert!(matches!(
        ensure(&mut CreateState::Prepared),
        Err(CloudError::IdentityMismatch)
    ));
    assert_eq!(ensure(&mut state).unwrap().id, 42);
    assert!(matches!(ensure(&mut state), Err(CloudError::WorkerLost)));
    assert!(matches!(
        ensure(&mut CreateState::Terminated { worker_id: "42".into() }),
        Err(CloudError::WorkerLost)
    ));
    task.join().unwrap();
    assert!(posts(&requests).is_empty());
}

#[test]
fn invalid_requests_never_reach_the_provider() {
    let placements = placements();
    let volume: Volume = serde_json::from_value(super::volumes::volume(9, None)).unwrap();
    let (hetzner, requests, task) = provider(vec![]);
    let cancel = Cancellation::default();
    let long = "x".repeat(32 * 1024 + 1);
    let invalid = [
        ServerRequest {
            operation_id: "Op_1",
            ..request(&placements)
        },
        ServerRequest {
            placements: &[],
            ..request(&placements)
        },
        ServerRequest {
            image: "docker ce",
            ..request(&placements)
        },
        ServerRequest {
            user_data: "",
            ..request(&placements)
        },
        ServerRequest {
            user_data: &long,
            ..request(&placements)
        },
        // The second placement is in another location than the volume.
        ServerRequest {
            volume: Some(&volume),
            ..request(&placements)
        },
    ];
    for request in &invalid {
        let mut state = CreateState::Prepared;
        assert!(matches!(
            hetzner.ensure_server(request, &mut state, &cancel, |_| Ok(()), |_| {}),
            Err(CloudError::Invalid(_))
        ));
        assert_eq!(state, CreateState::Prepared);
    }
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn deletion_verifies_identity_waits_for_the_action_and_proves_absence() {
    let (hetzner, requests, task) = provider(vec![
        (200, json!({"server": server(42)})),
        (200, json!({"action": action(5, "running")})),
        // Hetzner can keep the delete action running after the server is gone.
        (200, json!({"server": server(42)})),
        (200, json!({"action": action(5, "running")})),
        (404, error("not_found", "server not found")),
        (404, error("not_found", "server not found")),
        (404, error("not_found", "server not found")),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Bound { worker_id: "42".into() };
    let mut reported = Vec::new();
    hetzner
        .delete_server(OPERATION, &mut state, &cancel, |_| Ok(()), |step| reported.push(step))
        .unwrap();
    assert_eq!(state, CreateState::Terminated { worker_id: "42".into() });
    assert_eq!(
        reported,
        [
            Progress::ConfirmingWorker,
            Progress::Terminating,
            Progress::ConfirmingTermination
        ]
    );
    let mut state = CreateState::Bound { worker_id: "42".into() };
    reported.clear();
    hetzner
        .delete_server(OPERATION, &mut state, &cancel, |_| Ok(()), |step| reported.push(step))
        .unwrap();
    assert_eq!(
        reported,
        [Progress::ConfirmingWorker],
        "an absent server is never reported as being deleted"
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("DELETE /servers/42 "));
    assert!(requests[2].starts_with("GET /servers/42 "));
    assert!(requests[3].starts_with("GET /actions/5 "));
    assert!(requests[4].starts_with("GET /servers/42 "));
    assert!(matches!(
        hetzner.delete_server(OPERATION, &mut CreateState::Requested, &cancel, |_| Ok(()), |_| {}),
        Err(CloudError::CreationUnresolved)
    ));
}

#[test]
fn power_actions_act_only_on_the_operations_server() {
    let mut foreign = server(42);
    foreign["labels"] = json!({"horizon-operation": "other"});
    let (hetzner, requests, task) = provider(vec![
        (200, json!({"server": server(42)})),
        (201, json!({"action": action(6, "success")})),
        (200, json!({"server": server(42)})),
        (201, json!({"action": action(7, "success")})),
        (200, json!({"server": foreign})),
    ]);
    let cancel = Cancellation::default();
    hetzner.shutdown(OPERATION, 42, &cancel).unwrap();
    hetzner.power_off(OPERATION, 42, &cancel).unwrap();
    assert!(matches!(
        hetzner.power_on(OPERATION, 42, &cancel),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("POST /servers/42/actions/shutdown "));
    assert!(requests[3].starts_with("POST /servers/42/actions/poweroff "));
    assert_eq!(requests.len(), 5);
}

#[test]
fn provider_states_map_to_worker_states() {
    let mut value = server(42);
    for (status, expected) in [
        ("running", WorkerStatus::Running),
        ("initializing", WorkerStatus::Starting),
        ("starting", WorkerStatus::Starting),
        ("off", WorkerStatus::Stopped),
        ("stopping", WorkerStatus::Starting),
        ("deleting", WorkerStatus::Lost),
        ("unknown", WorkerStatus::Lost),
    ] {
        value["status"] = json!(status);
        assert_eq!(
            serde_json::from_value::<Server>(value.clone()).unwrap().status(),
            expected,
            "{status}"
        );
    }
    value["status"] = json!("running");
    value["public_net"]["ipv4"] = Value::Null;
    let server: Server = serde_json::from_value(value).unwrap();
    assert_eq!(server.status(), WorkerStatus::Starting);
    assert!(server.ssh_address().is_none());
}

#[test]
fn a_placement_error_also_moves_on_and_the_body_attaches_the_volume() {
    let placements = [("cpx22", "hel1"), ("cpx32", "hel1")].map(|(server_type, location)| Placement {
        server_type: server_type.into(),
        location: location.into(),
    });
    let volume: Volume = serde_json::from_value(super::volumes::volume(9, None)).unwrap();
    let mut holding = created(42);
    holding["server"]["volumes"] = json!([9]);
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (200, json!({"volume": super::volumes::volume(9, None)})),
        (422, error("placement_error", "error during placement")),
        (201, holding),
    ]);
    let request = ServerRequest {
        volume: Some(&volume),
        ..request(&placements)
    };
    let mut state = CreateState::Prepared;
    hetzner
        .ensure_server(&request, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    task.join().unwrap();
    let bodies = posts(&requests);
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[1]["server_type"], "cpx32");
    assert_eq!(bodies[1]["volumes"], json!([9]));
    assert_eq!(bodies[1]["automount"], false);
}

#[test]
fn cancellation_or_a_failed_fence_before_sending_never_posts() {
    let placements = placements();
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (200, listing("servers", json!([]))),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Prepared;
    let result = hetzner.ensure_server(
        &request(&placements),
        &mut state,
        &cancel,
        |_| Ok(()),
        |step| {
            if step == Progress::Requesting {
                cancel.cancel();
            }
        },
    );
    assert!(matches!(result, Err(CloudError::Cancelled)));
    assert_eq!(state, CreateState::Prepared);
    let mut state = CreateState::Prepared;
    assert!(matches!(
        hetzner.ensure_server(
            &request(&placements),
            &mut state,
            &Cancellation::default(),
            |_| Err(CloudError::Persistence),
            |_| {}
        ),
        Err(CloudError::Persistence)
    ));
    assert_eq!(state, CreateState::Prepared);
    task.join().unwrap();
    assert!(posts(&requests).is_empty());
}

#[test]
fn a_failed_create_action_keeps_the_server_bound_for_deletion() {
    let placements = placements();
    let mut failed = created(42);
    failed["action"] = json!({"id": 1, "status": "error", "error": {"code": "x", "message": "image unavailable"}});
    let (hetzner, _, task) = provider(vec![(200, listing("servers", json!([]))), (201, failed)]);
    let mut state = CreateState::Prepared;
    let error = hetzner
        .ensure_server(
            &request(&placements),
            &mut state,
            &Cancellation::default(),
            |_| Ok(()),
            |_| {},
        )
        .unwrap_err();
    task.join().unwrap();
    assert_eq!(
        error.to_string(),
        "Provider rejected the request or capacity is unavailable: image unavailable"
    );
    assert_eq!(state, CreateState::Bound { worker_id: "42".into() });
}

#[test]
fn a_server_still_present_after_deletion_stays_bound() {
    let (hetzner, _, task) = provider(vec![
        (200, json!({"server": server(42)})),
        (200, json!({"action": action(5, "success")})),
        (200, json!({"server": server(42)})),
    ]);
    let mut state = CreateState::Bound { worker_id: "42".into() };
    assert!(matches!(
        hetzner.delete_server(OPERATION, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::Invalid("Termination pending; reconcile again"))
    ));
    task.join().unwrap();
    assert_eq!(state, CreateState::Bound { worker_id: "42".into() });
}

#[test]
fn an_owned_server_in_the_wrong_place_is_bound_but_not_accepted() {
    let placements = placements();
    let volume: Volume = serde_json::from_value(super::volumes::volume(9, None)).unwrap();
    let hel1_only = [placements[0].clone()];
    let mut elsewhere = server(42);
    elsewhere["location"]["name"] = json!("fsn1");
    let mut other_type = server(42);
    other_type["server_type"]["name"] = json!("cpx32");
    let mut extra_volume = server(42);
    extra_volume["volumes"] = json!([9, 10]);
    let mut stray_volume = server(42);
    stray_volume["volumes"] = json!([10]);
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([server(42)]))),
        (200, listing("servers", json!([extra_volume]))),
        (200, listing("servers", json!([elsewhere]))),
        (200, listing("servers", json!([other_type]))),
        (200, listing("servers", json!([stray_volume]))),
    ]);
    let cancel = Cancellation::default();
    let with_volume = ServerRequest {
        volume: Some(&volume),
        ..request(&hel1_only)
    };
    let without_volume = request(&hel1_only);
    let mut state = CreateState::Prepared;
    let volume_error = "The operation's server does not hold exactly its workspace volume";
    let placement_error = "The operation's server has a type or location the request does not allow";
    let mut ensure = |request: &ServerRequest<'_>| {
        state = CreateState::Prepared;
        let error = hetzner
            .ensure_server(request, &mut state, &cancel, |_| Ok(()), |_| {})
            .unwrap_err()
            .to_string();
        assert_eq!(
            state,
            CreateState::Bound { worker_id: "42".into() },
            "still owned, so it can be deleted"
        );
        error
    };
    assert_eq!(ensure(&with_volume), volume_error, "the volume is missing");
    assert_eq!(ensure(&with_volume), volume_error, "an extra volume is attached");
    assert_eq!(ensure(&without_volume), placement_error, "another location");
    assert_eq!(ensure(&without_volume), placement_error, "another server type");
    assert_eq!(ensure(&without_volume), volume_error, "a volume nobody requested");
    task.join().unwrap();
    assert!(posts(&requests).is_empty());
}

#[test]
fn a_stale_volume_is_checked_live_and_the_key_goes_into_the_create_body() {
    let placements = placements();
    let volume: Volume = serde_json::from_value(super::volumes::volume(9, None)).unwrap();
    let key: SshKey = serde_json::from_value(super::keys::key(5)).unwrap();
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (200, json!({"volume": super::volumes::volume(9, Some(77))})),
        (200, json!({"server": server(77)})),
        (200, listing("servers", json!([]))),
        (201, created(42)),
    ]);
    let cancel = Cancellation::default();
    let hel1_only = [placements[0].clone()];
    let with_volume = ServerRequest {
        volume: Some(&volume),
        ..request(&hel1_only)
    };
    let mut state = CreateState::Prepared;
    assert_eq!(
        hetzner
            .ensure_server(&with_volume, &mut state, &cancel, |_| Ok(()), |_| {})
            .unwrap_err()
            .to_string(),
        "The workspace volume is attached to another server"
    );
    assert_eq!(state, CreateState::Prepared);
    let with_key = ServerRequest {
        ssh_key: Some(&key),
        ..request(&placements)
    };
    hetzner
        .ensure_server(&with_key, &mut state, &cancel, |_| Ok(()), |_| {})
        .unwrap();
    task.join().unwrap();
    let bodies = posts(&requests);
    assert_eq!(bodies.len(), 1, "no server is created for a volume held elsewhere");
    assert_eq!(bodies[0]["ssh_keys"], json!([5]));
}

#[test]
fn creation_waits_for_the_follow_up_start() {
    let placements = placements();
    let mut starting = created(42);
    starting["next_actions"] = json!([action(2, "running")]);
    let (hetzner, requests, task) = provider(vec![
        (200, listing("servers", json!([]))),
        (201, starting),
        (200, json!({"action": action(2, "success")})),
    ]);
    let mut state = CreateState::Prepared;
    hetzner
        .ensure_server(
            &request(&placements),
            &mut state,
            &Cancellation::default(),
            |_| Ok(()),
            |_| {},
        )
        .unwrap();
    task.join().unwrap();
    assert!(requests.lock().unwrap()[2].starts_with("GET /actions/2 "));
}
