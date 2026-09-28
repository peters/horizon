use super::*;
use crate::{
    CreateState,
    hetzner::volumes::{Volume, growth::Growth},
};

fn original() -> Volume {
    serde_json::from_value(volumes::volume(9, Some(42))).unwrap()
}
fn intent() -> Growth {
    Growth::new(
        OPERATION,
        &CreateState::Bound { worker_id: "9".into() },
        &original(),
        42,
        30,
    )
    .unwrap()
}
fn attached() -> Value {
    let mut server = servers::server(42);
    server["volumes"] = json!([9]);
    json!({"server":server})
}
fn observed(size: u32) -> Value {
    let mut volume = original();
    volume.size = size;
    json!({"volume": volume})
}

#[test]
fn growth_is_fenced_and_confirmed_without_repeating_the_resize() {
    let (client, requests, task) = provider(vec![
        (200, observed(10)),
        (200, attached()),
        (201, json!({"action": action(3, "running")})),
        (200, json!({"action": action(3, "success")})),
        (200, observed(30)),
        (200, attached()),
        (200, observed(30)),
        (200, attached()),
    ]);
    let mut growth = intent();
    let mut journal = Vec::new();
    let cancel = Cancellation::default();
    assert_eq!(
        client
            .grow_volume(&mut growth, &cancel, |next| {
                journal.push(serde_json::to_vec(next).unwrap());
                Ok(())
            })
            .unwrap()
            .size,
        30
    );
    assert!(growth.confirmed());
    assert_eq!(growth.requested_size(), 30);
    assert_eq!(journal.len(), 2);
    growth = serde_json::from_slice(&journal[1]).unwrap();
    client.grow_volume(&mut growth, &cancel, |_| Ok(())).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST ")).count(), 1);
    assert!(requests[2].starts_with("POST /volumes/9/actions/resize "));
    assert_eq!(request_body(&requests[2]), json!({"size":30}));
}

#[test]
fn lost_action_response_recovers_from_capacity_without_a_second_mutation() {
    let (client, requests, task) = provider(vec![
        (200, observed(10)),
        (200, attached()),
        (502, Value::Null),
        (200, observed(30)),
        (200, attached()),
    ]);
    let mut growth = intent();
    let mut saved = Vec::new();
    let cancel = Cancellation::default();
    assert!(
        client
            .grow_volume(&mut growth, &cancel, |next| {
                saved = serde_json::to_vec(next).unwrap();
                Ok(())
            })
            .is_err()
    );
    assert!(!growth.confirmed());
    growth = serde_json::from_slice(&saved).unwrap();
    client.grow_volume(&mut growth, &cancel, |_| Ok(())).unwrap();
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
}

#[test]
fn failed_persistence_and_identity_drift_prevent_provider_mutation() {
    let (client, requests, task) = provider(vec![(200, observed(10)), (200, attached())]);
    assert!(matches!(
        client.grow_volume(&mut intent(), &Cancellation::default(), |_| Err(
            CloudError::Persistence
        )),
        Err(CloudError::Persistence)
    ));
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 2);
    for field in ["name", "location", "server", "linux_device", "labels", "size", "status"] {
        let mut changed = observed(10);
        changed["volume"][field] = match field {
            "location" => json!({"name":"nbg1"}),
            "server" => json!(43),
            "labels" => json!({"horizon-operation":"foreign"}),
            "size" => json!(20),
            _ => json!("changed"),
        };
        let (client, requests, task) = provider(vec![(200, changed)]);
        assert!(
            client
                .grow_volume(&mut intent(), &Cancellation::default(), |_| panic!("must not persist"))
                .is_err(),
            "{field}"
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn lost_unsent_resize_retries_the_same_absolute_capacity() {
    let (client, requests, task) = provider(vec![
        (200, observed(10)),
        (200, attached()),
        (502, Value::Null),
        (200, observed(10)),
        (200, attached()),
        (201, json!({"action":action(3,"success")})),
        (200, observed(30)),
        (200, attached()),
    ]);
    let cancel = Cancellation::default();
    let mut growth = intent();
    assert!(client.grow_volume(&mut growth, &cancel, |_| Ok(())).is_err());
    let mut growth: Growth = serde_json::from_slice(&serde_json::to_vec(&growth).unwrap()).unwrap();
    client.grow_volume(&mut growth, &cancel, |_| Ok(())).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    let posted: Vec<_> = requests
        .iter()
        .filter(|r| r.starts_with("POST "))
        .map(|r| request_body(r))
        .collect();
    assert_eq!(posted, [json!({"size":30}), json!({"size":30})]);
}

#[test]
fn stale_or_foreign_worker_attachments_never_authorize_growth() {
    for field in ["missing", "labels", "volumes", "location", "status"] {
        let mut server = attached();
        server["server"][field] = match field {
            "labels" => json!({"horizon-operation":"foreign"}),
            "volumes" => json!([]),
            "location" => json!({"name":"nbg1"}),
            _ => json!("off"),
        };
        let response = if field == "missing" {
            (404, Value::Null)
        } else {
            (200, server)
        };
        let (client, requests, task) = provider(vec![(200, observed(10)), response]);
        assert!(
            client
                .grow_volume(&mut intent(), &Cancellation::default(), |_| panic!(
                    "no intent may advance"
                ))
                .is_err()
        );
        task.join().unwrap();
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
    }
    let bound = CreateState::Bound { worker_id: "9".into() };
    assert!(Growth::new(OPERATION, &bound, &original(), 43, 30).is_err());
}

#[test]
fn shrinking_foreign_and_unbound_volumes_are_rejected_locally() {
    let bound = CreateState::Bound { worker_id: "9".into() };
    for size in [0, 9, 10, 10_241] {
        assert!(Growth::new(OPERATION, &bound, &original(), 42, size).is_err());
    }
    for state in [
        CreateState::Prepared,
        CreateState::Requested,
        CreateState::Terminated { worker_id: "9".into() },
    ] {
        assert!(Growth::new(OPERATION, &state, &original(), 42, 30).is_err());
    }
    assert!(Growth::new("foreign", &bound, &original(), 42, 30).is_err());
}
