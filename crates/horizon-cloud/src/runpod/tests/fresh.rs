use crate::runpod::tests::{server, spec as base_spec, worker};
use crate::runpod::volumes::{Spec, State, Volume};
use crate::{Cancellation, CloudError, CreateState, WorkerSpec};
use serde_json::{Value, json};

fn spec() -> WorkerSpec {
    let mut spec = base_spec();
    spec.profile.gpu = false;
    spec.cpu_flavors = vec!["cpu3c".into()];
    spec.profile.cpu = 2;
    spec.profile.memory_gb = 4;
    spec.profile.storage.volume_gb = 10;
    spec.profile.storage.container_gb = 10;
    spec.startup_metadata = Some(crate::StartupMetadata::new("synthetic-context".into()).unwrap());
    spec
}
fn volume_spec() -> Spec {
    Spec {
        operation_id: spec().operation_id,
        size: 10,
        data_center_id: "test-region".into(),
    }
}
fn volume() -> Volume {
    let spec = volume_spec();
    Volume {
        id: "volume1".into(),
        name: spec.name(),
        size: spec.size,
        data_center_id: spec.data_center_id,
    }
}
fn observed() -> Value {
    let spec = spec();
    let mut value = worker(&spec);
    value["env"] = json!({"HORIZON_CLOUD_OPERATION":spec.operation_id,"HORIZON_WORKER_STARTUP":"synthetic-context"});
    value
}
fn mount() -> Value {
    json!({"id":"worker1", "name":spec().name(), "image":spec().image_digest,
        "dataCenterId":"test-region", "mounts":{"network":[{"volumeId":"volume1","path":"/workspace"}]}})
}
fn creation_responses() -> Vec<(u16, String)> {
    vec![
        (200, "[]".into()),
        (201, serde_json::to_string(&volume()).unwrap()),
        (200, "[]".into()),
        (200, "[]".into()),
        (201, observed().to_string()),
    ]
}

#[test]
fn direct_creation_and_exact_first_attachment_produce_a_consumable_witness() {
    let mut responses = creation_responses();
    responses.extend([
        (200, observed().to_string()),
        (200, mount().to_string()),
        (200, json!([observed()]).to_string()),
    ]);
    let (mut provider, requests, task) = server(responses);
    provider.api_endpoint = provider.endpoint.clone();
    let cancel = Cancellation::default();
    let mut volume_states = Vec::new();
    let fresh = provider
        .create_fresh_volume(&volume_spec(), &cancel, |next| {
            volume_states.push(next.clone());
            Ok(())
        })
        .unwrap();
    let mut worker_states = Vec::new();
    let created = fresh
        .create_worker(&spec(), &cancel, |next| {
            worker_states.push(next.clone());
            Ok(())
        })
        .unwrap();
    let attachment = created.confirm(&cancel).unwrap();
    assert_eq!(attachment.worker().id, "worker1");
    assert_eq!(attachment.volume(), &volume());
    assert_eq!(attachment.spec(), &spec());
    assert!(matches!(
        volume_states.as_slice(),
        [State::Requested, State::Bound { creation: Some(_), .. }]
    ));
    assert!(matches!(
        worker_states.as_slice(),
        [CreateState::Requested, CreateState::Bound { .. }]
    ));
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        2
    );
}

#[test]
fn an_existing_matching_worker_is_never_a_direct_creation_witness() {
    let mut responses = creation_responses();
    responses.truncate(3);
    responses.push((200, json!([observed()]).to_string()));
    let (provider, requests, task) = server(responses);
    let cancel = Cancellation::default();
    let fresh = provider
        .create_fresh_volume(&volume_spec(), &cancel, |_| Ok(()))
        .unwrap();
    assert!(matches!(
        fresh.create_worker(&spec(), &cancel, |_| panic!("adoption persisted")),
        Err(CloudError::IdentityMismatch)
    ));
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
fn uncertain_creation_and_failed_persistence_never_return_a_witness() {
    for failure in 0..3 {
        let mut responses = creation_responses();
        if failure == 0 {
            responses[4] = (503, "uncertain".into());
        }
        if failure == 1 {
            responses.truncate(4);
        }
        let (provider, requests, task) = server(responses);
        let cancel = Cancellation::default();
        let fresh = provider
            .create_fresh_volume(&volume_spec(), &cancel, |_| Ok(()))
            .unwrap();
        assert!(
            fresh
                .create_worker(&spec(), &cancel, |state| {
                    if (failure == 1 && *state == CreateState::Requested)
                        || (failure == 2 && matches!(state, CreateState::Bound { .. }))
                    {
                        Err(CloudError::Persistence)
                    } else {
                        Ok(())
                    }
                })
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            if failure == 1 { 1 } else { 2 }
        );
    }
}

#[test]
fn changed_mount_or_another_attachment_cannot_qualify_created_resources() {
    for sibling in [false, true] {
        let mut responses = creation_responses();
        let mut current = mount();
        if !sibling {
            current["mounts"]["network"][0]["volumeId"] = json!("retained-other-volume");
        }
        responses.extend([(200, observed().to_string()), (200, current.to_string())]);
        if sibling {
            let mut other = observed();
            other["id"] = json!("sibling");
            other["networkVolume"] = json!({"id":"volume1"});
            responses.push((200, json!([observed(), other]).to_string()));
        }
        let (mut provider, _, task) = server(responses);
        provider.api_endpoint = provider.endpoint.clone();
        let cancel = Cancellation::default();
        let created = provider
            .create_fresh_volume(&volume_spec(), &cancel, |_| Ok(()))
            .unwrap()
            .create_worker(&spec(), &cancel, |_| Ok(()))
            .unwrap();
        assert!(created.confirm(&cancel).is_err());
        task.join().unwrap();
    }
}
