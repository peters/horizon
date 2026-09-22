use super::*;

fn assigned(spec: &WorkerSpec) -> Value {
    let mut value = worker(spec);
    value["vcpuCount"] = json!(spec.profile.cpu);
    value["memoryInGb"] = json!(spec.profile.memory_gb);
    value["gpuCount"] = json!(1);
    value["containerDiskInGb"] = json!(spec.profile.storage.container_gb);
    value["volumeInGb"] = json!(spec.profile.storage.volume_gb);
    value["volumeMountPath"] = json!("/workspace");
    value["networkVolume"] = Value::Null;
    value
}

fn check(value: &Value, spec: &WorkerSpec) -> Result<(), CloudError> {
    serde_json::from_value::<Worker>(value.clone())
        .unwrap()
        .verify_resources(spec)
}

#[test]
fn both_compute_profiles_require_confirmed_storage_capacity_and_mount() {
    for gpu in [false, true] {
        let mut spec = spec();
        spec.profile.gpu = gpu;
        let request = create_body(&spec);
        assert_eq!(request["containerDiskInGb"], spec.profile.storage.container_gb);
        assert_eq!(request["volumeInGb"], spec.profile.storage.volume_gb);
        assert_eq!(request["volumeMountPath"], "/workspace");
        assert!(request.get("networkVolumeId").is_none());
        let value = assigned(&spec);
        assert!(check(&value, &spec).is_ok());
        for field in ["containerDiskInGb", "volumeInGb", "volumeMountPath"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(check(&missing, &spec).is_err(), "missing {field}, gpu={gpu}");
            missing[field] = Value::Null;
            assert!(check(&missing, &spec).is_err(), "null {field}, gpu={gpu}");
        }
        for (field, requested) in [
            ("containerDiskInGb", spec.profile.storage.container_gb),
            ("volumeInGb", spec.profile.storage.volume_gb),
        ] {
            for size in [0, requested - 1] {
                let mut undersized = value.clone();
                undersized[field] = json!(size);
                assert!(check(&undersized, &spec).is_err(), "{field}={size}, gpu={gpu}");
            }
            let mut larger = value.clone();
            larger[field] = json!(u32::from(requested) + 1);
            assert!(check(&larger, &spec).is_ok());
        }
        for attachment in [
            json!({}),
            json!({"id":"unrequested-volume","size":1000,"dataCenterId":"test-region"}),
        ] {
            let mut attached = value.clone();
            attached["networkVolume"] = attachment;
            assert!(check(&attached, &spec).is_err());
            serde_json::from_value::<Worker>(attached)
                .unwrap()
                .verify(&spec)
                .unwrap();
        }
        for path in ["", "/", "/different-volume"] {
            let mut wrong_mount = value.clone();
            wrong_mount["volumeMountPath"] = json!(path);
            assert!(check(&wrong_mount, &spec).is_err());
        }
    }
}

#[test]
fn old_worker_records_remain_readable_but_cannot_certify_storage() {
    let spec = spec();
    let mut legacy = assigned(&spec);
    for field in ["containerDiskInGb", "volumeInGb", "volumeMountPath", "networkVolume"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let old: Worker = serde_json::from_value(legacy).unwrap();
    assert!(old.container_disk_in_gb.is_none());
    assert!(old.volume_in_gb.is_none());
    assert!(old.volume_mount_path.is_none());
    assert!(old.network_volume.is_none());
    old.verify(&spec).unwrap();
    assert!(old.verify_resources(&spec).is_err());
    let confirmed: Worker = serde_json::from_value(assigned(&spec)).unwrap();
    let persisted = serde_json::to_value(confirmed).unwrap();
    assert!(check(&persisted, &spec).is_ok());
}

#[test]
fn rejected_storage_keeps_allocation_bound_and_explicit_deletion_available() {
    let spec = spec();
    let mut inadequate = assigned(&spec);
    inadequate["volumeInGb"] = json!(0);
    let body = inadequate.to_string();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (201, body.clone()),
        (200, body.clone()),
        (200, body),
        (204, String::new()),
        (404, "{}".into()),
    ]);
    let mut state = CreateState::Prepared;
    let mut persisted = Vec::new();
    let created = provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |next| {
                persisted.push(next.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    assert!(created.verify_resources(&spec).is_err());
    let bound = CreateState::Bound {
        worker_id: "worker1".into(),
    };
    assert_eq!(state, bound);
    assert_eq!(persisted, vec![CreateState::Requested, bound.clone()]);
    let retried = provider
        .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    assert!(retried.verify_resources(&spec).is_err());
    assert_eq!(state, bound);
    provider
        .terminate(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert!(matches!(state, CreateState::Terminated { .. }));
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /pods ")).count(), 1);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("DELETE /pods/worker1 "))
            .count(),
        1
    );
}

fn network_spec() -> WorkerSpec {
    let mut spec = spec();
    spec.network_volume = Some(crate::NetworkVolumeBinding {
        id: "volume1".into(),
        data_center_id: "region1".into(),
    });
    spec
}

fn network_volume(spec: &WorkerSpec) -> Value {
    json!({"id":"volume1", "dataCenterId":"region1", "size":spec.profile.storage.volume_gb})
}

#[test]
fn explicit_network_storage_requires_confirmed_attachment_for_both_compute_types() {
    for gpu in [false, true] {
        let mut spec = network_spec();
        spec.profile.gpu = gpu;
        spec.validate().unwrap();
        let request = create_body(&spec);
        assert_eq!(request["networkVolumeId"], "volume1");
        assert_eq!(request["dataCenterIds"], json!(["region1"]));
        assert_eq!(request["volumeInGb"], 0);
        let mut value = assigned(&spec);
        value["volumeInGb"] = json!(0);
        value["networkVolume"] = network_volume(&spec);
        assert!(check(&value, &spec).is_ok());
        for attachment in [Value::Null, json!({}), json!({"id":"volume1"})] {
            let mut missing = value.clone();
            missing["networkVolume"] = attachment;
            assert!(check(&missing, &spec).is_err());
        }
        for (field, invalid) in [
            ("id", json!("other")),
            ("dataCenterId", json!("elsewhere")),
            ("size", json!(0)),
        ] {
            let mut mismatch = value.clone();
            mismatch["networkVolume"][field] = invalid;
            assert!(check(&mismatch, &spec).is_err());
        }
        for field in ["id", "dataCenterId", "size"] {
            let mut missing = value.clone();
            missing["networkVolume"].as_object_mut().unwrap().remove(field);
            assert!(check(&missing, &spec).is_err());
        }
        for (field, invalid) in [("containerDiskInGb", json!(0)), ("volumeMountPath", json!("/wrong"))] {
            let mut mismatch = value.clone();
            mismatch[field] = invalid;
            assert!(check(&mismatch, &spec).is_err());
        }
        value["networkVolume"]["size"] = json!(u32::from(spec.profile.storage.volume_gb) + 1);
        assert!(check(&value, &spec).is_ok());
    }
}

#[test]
fn network_preflight_failure_never_records_or_sends_a_create() {
    let spec = network_spec();
    for (status, volume) in [
        (404, json!({})),
        (401, json!({})),
        (200, json!({})),
        (200, json!({"id":"other", "dataCenterId":"region1", "size":1000})),
        (200, json!({"id":"volume1", "dataCenterId":"elsewhere", "size":1000})),
        (200, json!({"id":"volume1", "dataCenterId":"region1", "size":0})),
    ] {
        let (provider, requests, task) = server(vec![(200, "[]".into()), (status, volume.to_string())]);
        let mut state = CreateState::Prepared;
        assert!(
            provider
                .ensure(
                    &spec,
                    &mut state,
                    &Cancellation::default(),
                    |_| panic!("must not persist a create"),
                    |_| {}
                )
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(state, CreateState::Prepared);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET /networkvolumes/volume1 "));
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
    }
}

#[test]
fn network_create_is_fenced_and_worker_deletion_never_deletes_storage() {
    let spec = network_spec();
    let mut assigned = assigned(&spec);
    assigned["volumeInGb"] = json!(0);
    assigned["networkVolume"] = network_volume(&spec);
    let body = assigned.to_string();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (200, network_volume(&spec).to_string()),
        (201, body.clone()),
        (200, body.clone()),
        (200, body),
        (204, String::new()),
        (404, "{}".into()),
    ]);
    let mut state = CreateState::Prepared;
    let mut persisted = Vec::new();
    let worker = provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |next| {
                persisted.push(next.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    worker.verify_resources(&spec).unwrap();
    assert_eq!(
        persisted,
        vec![
            CreateState::Requested,
            CreateState::Bound {
                worker_id: "worker1".into()
            }
        ]
    );
    provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |_| panic!("bound operation remains bound"),
            |_| {},
        )
        .unwrap();
    provider
        .terminate(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(
        state,
        CreateState::Terminated {
            worker_id: "worker1".into()
        }
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /pods?includeNetworkVolume=true&includeWorkers=true "));
    assert!(requests[3].starts_with("GET /pods/worker1?includeNetworkVolume=true "));
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("POST ")).count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /networkvolumes/"))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .filter(|request| request.starts_with("DELETE "))
            .all(|request| request.starts_with("DELETE /pods/worker1 "))
    );
}

#[test]
fn uncertain_network_create_only_reconciles_and_legacy_specs_stay_unbound() {
    let spec = network_spec();
    let (provider, requests, task) = server(vec![(200, "[]".into())]);
    let mut state = CreateState::Requested;
    assert!(matches!(
        provider.ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |_| panic!("uncertain fence stays"),
            |_| {}
        ),
        Err(CloudError::CreationUnresolved)
    ));
    task.join().unwrap();
    assert_eq!(state, CreateState::Requested);
    assert_eq!(requests.lock().unwrap().len(), 1);
    let mut legacy = serde_json::to_value(&spec).unwrap();
    legacy.as_object_mut().unwrap().remove("network_volume");
    let restored: WorkerSpec = serde_json::from_value(legacy).unwrap();
    assert!(restored.network_volume.is_none());
    assert_eq!(
        serde_json::from_value::<WorkerSpec>(serde_json::to_value(&spec).unwrap()).unwrap(),
        spec
    );
}

#[test]
fn network_binding_rejects_malformed_identifiers_and_location_conflicts() {
    let mut spec = network_spec();
    spec.data_centers = vec!["another-region".into()];
    assert!(spec.validate().is_err());
    spec.data_centers.push("region1".into());
    spec.validate().unwrap();
    assert_eq!(create_body(&spec)["dataCenterIds"], json!(["region1"]));
    for invalid in ["", "../wrong", "contains space"] {
        spec.network_volume.as_mut().unwrap().id = invalid.into();
        assert!(spec.validate().is_err());
        spec.network_volume = Some(crate::NetworkVolumeBinding {
            id: "volume1".into(),
            data_center_id: invalid.into(),
        });
        assert!(spec.validate().is_err());
        spec.network_volume = network_spec().network_volume;
    }
}

#[test]
fn network_volume_attached_to_another_worker_blocks_new_allocation() {
    let spec = network_spec();
    let mut other = assigned(&spec);
    other["name"] = json!("another-operation");
    other["networkVolume"] = network_volume(&spec);
    let (provider, requests, task) = server(vec![(200, json!([other]).to_string())]);
    let mut state = CreateState::Prepared;
    assert!(
        provider
            .ensure(
                &spec,
                &mut state,
                &Cancellation::default(),
                |_| panic!("must not allocate"),
                |_| {}
            )
            .is_err()
    );
    task.join().unwrap();
    assert_eq!(state, CreateState::Prepared);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn rejected_network_attachment_does_not_block_identity_safe_worker_cleanup() {
    let spec = network_spec();
    let mut wrong = assigned(&spec);
    wrong["networkVolume"] = json!({"id":"unexpected", "size":0, "dataCenterId":"wrong"});
    assert!(check(&wrong, &spec).is_err());
    let (provider, requests, task) = server(vec![(200, wrong.to_string()), (204, String::new()), (404, "{}".into())]);
    let mut state = CreateState::Bound {
        worker_id: "worker1".into(),
    };
    provider
        .terminate(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    task.join().unwrap();
    assert_eq!(
        state,
        CreateState::Terminated {
            worker_id: "worker1".into()
        }
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.contains("/networkvolumes/"))
    );
}
