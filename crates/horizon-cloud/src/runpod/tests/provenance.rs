use super::*;
use crate::runpod::volumes::{Spec, State, Volume};

fn volume_spec() -> Spec {
    Spec {
        operation_id: "synthetic-creation".into(),
        size: 20,
        data_center_id: "test-region".into(),
    }
}
fn volume() -> Volume {
    let spec = volume_spec();
    Volume {
        id: "test-volume".into(),
        name: spec.name(),
        size: spec.size,
        data_center_id: spec.data_center_id,
    }
}
fn response() -> String {
    serde_json::to_string(&volume()).unwrap()
}
fn ensure(provider: &RunPod, state: &mut State) {
    assert_eq!(
        provider
            .ensure_volume(&volume_spec(), state, &Cancellation::default(), |_| Ok(()))
            .unwrap(),
        volume()
    );
}

#[test]
fn direct_creation_receipt_survives_persistence_and_read_only_inspection() {
    let (provider, requests, task) = server(vec![
        (200, endpoints(&json!([]))),
        (200, volumes(&json!([]))),
        (201, response()),
        (200, response()),
    ]);
    let mut state = State::Prepared;
    let mut durable = Vec::new();
    provider
        .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
            durable = serde_json::to_vec(next).unwrap();
            Ok(())
        })
        .unwrap();
    assert!(state.creation_receipt(&volume_spec()).unwrap().is_some());
    let mut reopened: State = serde_json::from_slice(&durable).unwrap();
    ensure(&provider, &mut reopened);
    assert_eq!(reopened, state);
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
fn uncertain_response_and_failed_receipt_persistence_never_promote_reconciliation() {
    for fail_persistence in [false, true] {
        let (provider, requests, task) = server(vec![
            (200, endpoints(&json!([]))),
            (200, volumes(&json!([]))),
            if fail_persistence {
                (201, response())
            } else {
                (503, "uncertain".into())
            },
            (200, volumes(&json!([volume()]))),
            (200, response()),
        ]);
        let mut state = State::Prepared;
        let mut durable = Vec::new();
        let result = provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
            if matches!(next, State::Bound { .. }) {
                return Err(CloudError::Persistence);
            }
            durable = serde_json::to_vec(next).unwrap();
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(state, State::Requested);
        let mut reopened: State = serde_json::from_slice(&durable).unwrap();
        ensure(&provider, &mut reopened);
        assert!(reopened.creation_receipt(&volume_spec()).unwrap().is_none());
        ensure(&provider, &mut reopened);
        assert!(reopened.creation_receipt(&volume_spec()).unwrap().is_none());
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
}

#[test]
fn legacy_bound_and_deleting_shapes_round_trip_without_creation_authority() {
    for stage in ["bound", "deleting"] {
        let original = json!({"state":stage,"volume":volume()});
        let state: State = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), original);
        assert!(state.creation_receipt(&volume_spec()).unwrap().is_none());
    }
    let (provider, _, task) = server(vec![(200, response())]);
    let mut state = State::Bound {
        volume: volume(),
        creation: None,
    };
    ensure(&provider, &mut state);
    assert!(state.creation_receipt(&volume_spec()).unwrap().is_none());
    task.join().unwrap();
}

#[test]
fn changed_receipt_or_volume_blocks_inspection_and_deletion_before_io() {
    let (provider, requests, task) = server(vec![
        (200, endpoints(&json!([]))),
        (200, volumes(&json!([]))),
        (201, response()),
    ]);
    let mut state = State::Prepared;
    ensure(&provider, &mut state);
    task.join().unwrap();
    let original = serde_json::to_value(state).unwrap();
    for (path, replacement) in [
        ("/creation/version", json!(2)),
        ("/creation/spec/operation_id", json!("other-operation")),
        ("/creation/spec/size", json!(40)),
        ("/creation/spec/data_center_id", json!("other-region")),
        ("/creation/volume/id", json!("other-volume")),
        ("/volume/id", json!("other-volume")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(path).unwrap() = replacement;
        let mut state: State = serde_json::from_value(changed).unwrap();
        assert!(matches!(
            state.creation_receipt(&volume_spec()),
            Err(CloudError::IdentityMismatch)
        ));
        assert!(matches!(
            provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                "invalid intent persisted"
            )),
            Err(CloudError::IdentityMismatch)
        ));
        assert!(matches!(
            provider.terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                "invalid deletion persisted"
            )),
            Err(CloudError::IdentityMismatch)
        ));
    }
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[test]
fn deletion_retains_creation_evidence_without_exposing_bootstrap_permission() {
    let (provider, _, task) = server(vec![
        (200, endpoints(&json!([]))),
        (200, volumes(&json!([]))),
        (201, response()),
        (200, response()),
        (200, endpoints(&json!([]))),
        (200, pods(&json!([]))),
        (503, "uncertain".into()),
        (404, String::new()),
    ]);
    let mut state = State::Prepared;
    ensure(&provider, &mut state);
    let original = state.creation_receipt(&volume_spec()).unwrap().cloned();
    let mut saved = Vec::new();
    assert!(
        provider
            .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
                saved = serde_json::to_vec(next).unwrap();
                Ok(())
            })
            .is_err()
    );
    let State::Deleting { creation, .. } = &state else {
        panic!("deletion fence missing")
    };
    assert_eq!(*creation, original);
    assert!(state.creation_receipt(&volume_spec()).unwrap().is_none());
    let mut reopened: State = serde_json::from_slice(&saved).unwrap();
    provider
        .terminate_volume(&volume_spec(), &mut reopened, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(reopened, State::Deleted);
    task.join().unwrap();
}
