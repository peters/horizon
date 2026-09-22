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
