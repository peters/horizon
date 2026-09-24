use super::*;
use crate::{StartupMetadata, startup::ENVIRONMENT_KEY};

fn configured() -> WorkerSpec {
    let mut spec = spec();
    spec.startup_metadata = Some(StartupMetadata::new(r#"{"version":1,"binding":"synthetic"}"#.into()).unwrap());
    spec
}

fn observed(spec: &WorkerSpec) -> Value {
    let mut value = worker(spec);
    value["env"] = json!({
        "HORIZON_CLOUD_OPERATION": spec.operation_id,
        ENVIRONMENT_KEY: spec.startup_metadata.as_ref().unwrap().as_str(),
    });
    value
}

#[test]
fn creation_persists_intent_before_transmitting_the_saved_metadata() {
    let spec = configured();
    let (provider, requests, task) = server(vec![(200, "[]".into()), (200, observed(&spec).to_string())]);
    let mut state = CreateState::Prepared;
    let mut transitions = Vec::new();
    provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |next| {
                transitions.push(next.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    task.join().unwrap();
    assert_eq!(
        transitions,
        [
            CreateState::Requested,
            CreateState::Bound {
                worker_id: "worker1".into()
            }
        ]
    );
    let requests = requests.lock().unwrap();
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["env"][ENVIRONMENT_KEY], spec.startup_metadata.unwrap().as_str());
    assert_eq!(body["env"]["HORIZON_CLOUD_OPERATION"], spec.operation_id);
}

#[test]
fn creation_without_confirmed_metadata_keeps_the_requested_fence() {
    let spec = configured();
    let (provider, requests, task) = server(vec![(200, "[]".into()), (200, worker(&spec).to_string())]);
    let mut state = CreateState::Prepared;
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    assert_eq!(state, CreateState::Requested);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn changed_metadata_after_a_lost_response_cannot_adopt_or_create_again() {
    let mut spec = configured();
    let original = observed(&spec);
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (500, "{}".into()),
        (200, json!([original]).to_string()),
    ]);
    let mut state = CreateState::Prepared;
    assert!(
        provider
            .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
            .is_err()
    );
    assert_eq!(state, CreateState::Requested);
    spec.startup_metadata = Some(StartupMetadata::new("changed-binding".into()).unwrap());
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    assert_eq!(state, CreateState::Requested);
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
fn absent_or_different_metadata_blocks_reconnect_and_provider_mutations() {
    for action in [
        "adopt",
        "reconcile",
        "reconnect",
        "stop",
        "start",
        "delete",
        "replace",
        "observe",
    ] {
        for missing in [false, true] {
            let spec = configured();
            let mut value = observed(&spec);
            if missing {
                value["env"].as_object_mut().unwrap().remove(ENVIRONMENT_KEY);
            } else {
                value["env"][ENVIRONMENT_KEY] = json!("different-binding");
            }
            let listed = matches!(action, "adopt" | "reconcile");
            let response = if listed { json!([value]) } else { value };
            let (provider, requests, task) = server(vec![(200, response.to_string())]);
            let mut state = match action {
                "adopt" => CreateState::Prepared,
                "reconcile" => CreateState::Requested,
                _ => CreateState::Bound {
                    worker_id: "worker1".into(),
                },
            };
            let previous = state.clone();
            let cancel = Cancellation::default();
            let result = match action {
                "stop" => provider.stop(&spec, "worker1", &cancel),
                "start" => provider.start(&spec, "worker1", &cancel),
                "delete" => provider.terminate(&spec, &mut state, &cancel, |_| Ok(())),
                "reconcile" => provider
                    .reconcile(&spec, &mut state, None, &cancel, |_| Ok(()))
                    .map(|_| ()),
                "replace" | "observe" => {
                    let mut next = spec.clone();
                    next.image_digest = format!("example/worker@sha256:{}", "b".repeat(64));
                    if action == "replace" {
                        provider.replace_image(&spec, &next, "worker1", &cancel)
                    } else {
                        provider
                            .observe_image("worker1", &spec, &next, None, &cancel, Duration::from_secs(2))
                            .map(|_| ())
                    }
                }
                _ => provider
                    .ensure(&spec, &mut state, &cancel, |_| Ok(()), |_| {})
                    .map(|_| ()),
            };
            assert!(matches!(result, Err(CloudError::IdentityMismatch)), "{action}");
            task.join().unwrap();
            assert_eq!(state, previous);
            assert_eq!(requests.lock().unwrap().len(), 1);
            assert!(requests.lock().unwrap()[0].starts_with("GET "));
        }
    }
}

#[test]
fn image_replacement_cannot_change_or_remove_immutable_startup_metadata() {
    let current = configured();
    for metadata in [None, Some(StartupMetadata::new("different-binding".into()).unwrap())] {
        let mut next = current.clone();
        next.image_digest = format!("example/worker@sha256:{}", "b".repeat(64));
        next.startup_metadata = metadata;
        let (provider, requests, task) = server(vec![]);
        let cancel = Cancellation::default();
        assert!(matches!(
            provider.replace_image(&current, &next, "worker1", &cancel),
            Err(CloudError::IdentityChange)
        ));
        assert!(matches!(
            provider.observe_image("worker1", &current, &next, None, &cancel, Duration::from_secs(2)),
            Err(CloudError::IdentityChange)
        ));
        task.join().unwrap();
        assert!(requests.lock().unwrap().is_empty());
    }
}

#[test]
fn metadata_enabled_workers_require_the_creation_operation_echo() {
    let spec = configured();
    let mut value = observed(&spec);
    value["env"].as_object_mut().unwrap().remove("HORIZON_CLOUD_OPERATION");
    let worker: Worker = serde_json::from_value(value).unwrap();
    assert!(matches!(worker.verify(&spec), Err(CloudError::IdentityMismatch)));
    let original: Worker = serde_json::from_value(observed(&spec)).unwrap();
    assert!(original.verify(&spec).is_ok());
    let mut without_metadata = spec;
    without_metadata.startup_metadata = None;
    assert!(matches!(
        original.verify(&without_metadata),
        Err(CloudError::IdentityMismatch)
    ));
}

#[test]
fn old_specs_preserve_the_original_request_and_legacy_identity_checks() {
    let spec = spec();
    let encoded = serde_json::to_value(&spec).unwrap();
    assert!(encoded.get("startup_metadata").is_none());
    let restored: WorkerSpec = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored, spec);
    assert!(create_body(&restored)["env"].get(ENVIRONMENT_KEY).is_none());
    let legacy: Worker = serde_json::from_value(worker(&spec)).unwrap();
    assert!(legacy.verify(&restored).is_ok());
}
