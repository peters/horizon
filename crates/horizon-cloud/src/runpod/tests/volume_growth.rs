use super::*;
use crate::runpod::volumes::{Spec, State, Tier, Volume, growth::Growth};

fn volume(size: u32) -> Volume {
    Volume {
        id: "volume1".into(),
        name: "horizon-volume-test-operation".into(),
        size,
        data_center_id: "test-region".into(),
        tier: Some(Tier::Standard),
    }
}
fn spec() -> Spec {
    serde_json::from_value(json!({"operation_id":"test-operation","size":20,"data_center_id":"test-region"})).unwrap()
}
fn intent() -> Growth {
    Growth::new(
        &spec(),
        &State::Bound {
            volume: volume(20),
            creation: None,
        },
        40,
    )
    .unwrap()
}
fn response(size: u32) -> (u16, String) {
    (200, serde_json::to_string(&volume(size)).unwrap())
}

#[test]
fn growth_is_fenced_and_confirmed_without_reallocation_or_creation_authority() {
    let (provider, requests, task) = server(vec![response(20), (204, String::new()), response(40), response(40)]);
    let mut growth = intent();
    let mut saved = Vec::new();
    let result = provider
        .grow_volume(&mut growth, &Cancellation::default(), |next| {
            saved.push(serde_json::to_vec(next).unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(result.size, 40);
    assert!(growth.confirmed());
    assert_eq!(saved.len(), 2);
    growth = serde_json::from_slice(&saved[1]).unwrap();
    provider
        .grow_volume(&mut growth, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("PATCH ")).count(), 1);
    assert!(requests.iter().all(|r| !r.starts_with("POST ")));
    let body: Value = serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"size":40}));
}

#[test]
fn lost_growth_response_recovers_by_reading_without_a_second_patch() {
    let (provider, requests, task) = server(vec![response(20), (500, String::new()), response(40)]);
    let mut growth = intent();
    let mut saved = Vec::new();
    assert!(
        provider
            .grow_volume(&mut growth, &Cancellation::default(), |next| {
                saved = serde_json::to_vec(next).unwrap();
                Ok(())
            })
            .is_err()
    );
    growth = serde_json::from_slice(&saved).unwrap();
    assert!(!growth.confirmed());
    provider
        .grow_volume(&mut growth, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert!(growth.confirmed());
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("PATCH "))
            .count(),
        1
    );
}

#[test]
fn persistence_failure_blocks_mutation_and_foreign_volume_is_never_patched() {
    let (provider, requests, task) = server(vec![response(20)]);
    assert!(matches!(
        provider.grow_volume(&mut intent(), &Cancellation::default(), |_| Err(
            CloudError::Persistence
        )),
        Err(CloudError::Persistence)
    ));
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
    for changed in ["id", "name", "dataCenterId", "type", "size"] {
        let mut value = serde_json::to_value(volume(20)).unwrap();
        value[changed] = match changed {
            "size" => json!(30),
            "type" => json!("HIGH_PERFORMANCE"),
            _ => json!("foreign"),
        };
        let (provider, requests, task) = server(vec![(200, value.to_string())]);
        assert!(
            provider
                .grow_volume(&mut intent(), &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn shrinking_missing_tier_and_deleting_storage_are_rejected_locally() {
    let spec = spec();
    for size in [0, 10, 20, 4001] {
        assert!(
            Growth::new(
                &spec,
                &State::Bound {
                    volume: volume(20),
                    creation: None
                },
                size
            )
            .is_err()
        );
    }
    let mut legacy = volume(20);
    legacy.tier = None;
    assert!(
        Growth::new(
            &spec,
            &State::Bound {
                volume: legacy,
                creation: None
            },
            40
        )
        .is_err()
    );
    assert!(
        Growth::new(
            &spec,
            &State::Deleting {
                volume: volume(20),
                creation: None
            },
            40
        )
        .is_err()
    );
}
