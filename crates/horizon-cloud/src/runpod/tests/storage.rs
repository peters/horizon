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

mod volumes {
    use super::*;
    use crate::runpod::volumes::{Spec, State, Volume};

    fn server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let (mut provider, requests, task) = super::server(responses);
        provider.api_endpoint.clone_from(&provider.endpoint);
        (provider, requests, task)
    }

    fn mounted_worker() -> Value {
        json!({"id":"worker1", "name":spec().name(), "image":spec().image_digest,
            "dataCenterId":volume().data_center_id,
            "mounts":{"network":[{"volumeId":volume().id,"path":"/workspace"}]}})
    }

    #[test]
    fn current_api_confirms_cpu_mount_missing_from_legacy_worker() {
        let (provider, requests, task) = server(vec![(200, mounted_worker().to_string())]);
        let mut worker: Worker = serde_json::from_value(worker(&spec())).unwrap();
        assert!(worker.network_volume.is_none());
        provider
            .confirm_workspace_mount(&mut worker, &volume(), &Cancellation::default(), Duration::from_secs(2))
            .unwrap();
        assert_eq!(worker.network_volume.unwrap().id, Some(volume().id));
        task.join().unwrap();
        assert!(requests.lock().unwrap()[0].starts_with("GET /pods/worker1 "));
    }

    #[test]
    fn current_mount_confirmation_refuses_identity_location_and_mount_drift() {
        let mut cases = Vec::new();
        for field in ["id", "name", "image", "dataCenterId"] {
            let mut value = mounted_worker();
            value[field] = json!("wrong");
            cases.push(value);
        }
        for mounts in [
            json!({}),
            json!({"network":[]}),
            json!({"network":[{"volumeId":"wrong","path":"/workspace"}]}),
            json!({"network":[{"volumeId":volume().id,"path":"/wrong"}]}),
            json!({"network":[{"volumeId":volume().id,"path":"/workspace"},{"volumeId":"other","path":"/other"}]}),
            json!({"persistent":{"size":20},"network":[{"volumeId":volume().id,"path":"/workspace"}]}),
        ] {
            let mut value = mounted_worker();
            value["mounts"] = mounts;
            cases.push(value);
        }
        for value in cases {
            let (provider, _, task) = server(vec![(200, value.to_string())]);
            let mut worker: Worker = serde_json::from_value(worker(&spec())).unwrap();
            assert!(
                provider
                    .confirm_workspace_mount(&mut worker, &volume(), &Cancellation::default(), Duration::from_secs(2))
                    .is_err()
            );
            assert!(worker.network_volume.is_none());
            task.join().unwrap();
        }
    }

    #[test]
    fn deletion_checks_current_cpu_mounts_even_when_legacy_attachment_is_absent() {
        let pod = worker(&spec());
        let (provider, requests, task) = server(vec![
            (200, value()),
            (200, json!([pod]).to_string()),
            (200, mounted_worker().to_string()),
        ]);
        let mut state = State::Bound { volume: volume() };
        assert!(
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
        assert!(matches!(state, State::Bound { .. }));
    }

    #[test]
    fn missing_current_worker_requires_legacy_absence_before_storage_deletion() {
        let pod = worker(&spec());
        let (provider, requests, task) = server(vec![
            (200, value()),
            (200, json!([pod]).to_string()),
            (404, String::new()),
            (200, pod.to_string()),
        ]);
        let mut state = State::Bound { volume: volume() };
        assert!(
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
        assert!(matches!(state, State::Bound { .. }));
    }

    #[test]
    fn optional_network_mount_list_is_absent_on_unattached_workers() {
        for mounts in [json!({}), json!({"persistent":{"size":20,"path":"/workspace"}})] {
            let mut current = mounted_worker();
            current["mounts"] = mounts;
            let (provider, requests, task) = server(vec![
                (200, value()),
                (200, json!([worker(&spec())]).to_string()),
                (200, current.to_string()),
                (204, String::new()),
                (404, String::new()),
            ]);
            let mut state = State::Bound { volume: volume() };
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .unwrap();
            task.join().unwrap();
            assert_eq!(state, State::Deleted);
            assert_eq!(
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|r| r.starts_with("DELETE "))
                    .count(),
                1
            );
        }
    }

    fn volume_spec() -> Spec {
        Spec {
            operation_id: spec().operation_id,
            size: u32::from(spec().profile.storage.volume_gb),
            data_center_id: "EU-TEST-1".into(),
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
    fn value() -> String {
        serde_json::to_string(&volume()).unwrap()
    }

    #[test]
    fn volume_allocation_is_fenced_and_bound_before_worker_request() {
        let (provider, requests, task) = server(vec![(200, "[]".into()), (201, value())]);
        let mut state = State::Prepared;
        let mut saved = Vec::new();
        provider
            .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
                if *next == State::Requested {
                    assert_eq!(requests.lock().unwrap().len(), 1);
                }
                saved.push(next.clone());
                Ok(())
            })
            .unwrap();
        task.join().unwrap();
        assert_eq!(saved, vec![State::Requested, State::Bound { volume: volume() }]);
        assert_eq!(state, saved[1]);
        assert!(requests.lock().unwrap()[1].starts_with("POST /networkvolumes "));
    }

    #[test]
    fn uncertain_volume_create_reconciles_without_a_second_post() {
        let (provider, requests, task) = server(vec![
            (200, "[]".into()),
            (503, "unavailable".into()),
            (200, json!([volume()]).to_string()),
        ]);
        let mut state = State::Prepared;
        assert!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        assert_eq!(state, State::Requested);
        assert_eq!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .unwrap(),
            volume()
        );
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
    fn empty_or_conflicting_evidence_never_resets_the_volume_fence() {
        for response in [json!([]), json!([volume(), volume()])] {
            let (provider, requests, task) = server(vec![(200, response.to_string())]);
            let mut state = State::Requested;
            assert!(
                provider
                    .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                    .is_err()
            );
            assert_eq!(state, State::Requested);
            task.join().unwrap();
            assert_eq!(requests.lock().unwrap().len(), 1);
        }
        let (provider, requests, task) = server(vec![(200, json!([volume()]).to_string())]);
        let mut state = State::Prepared;
        assert!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        assert_eq!(state, State::Prepared);
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn failed_volume_fence_persistence_prevents_allocation() {
        let (provider, requests, task) = server(vec![(200, "[]".into())]);
        let mut state = State::Prepared;
        assert!(matches!(
            provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Err(
                CloudError::Persistence
            )),
            Err(CloudError::Persistence)
        ));
        task.join().unwrap();
        assert_eq!(state, State::Prepared);
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn missing_bound_storage_is_never_replaced() {
        let (provider, requests, task) = server(vec![(404, String::new())]);
        let mut state = State::Bound { volume: volume() };
        assert!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(matches!(state, State::Bound { .. }));
    }

    #[test]
    fn volume_cleanup_retries_lost_delete_response_and_confirms_absence() {
        let (provider, requests, task) = server(vec![
            (200, value()),
            (200, "[]".into()),
            (503, String::new()),
            (404, String::new()),
        ]);
        let mut state = State::Bound { volume: volume() };
        assert!(
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        assert!(matches!(state, State::Deleting { .. }));
        provider
            .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        task.join().unwrap();
        assert_eq!(state, State::Deleted);
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("DELETE "))
                .count(),
            1
        );
    }

    #[test]
    fn incomplete_attachment_evidence_prevents_storage_deletion() {
        for attachment in [
            json!({}),
            json!({"id":null}),
            json!({"id":""}),
            json!({"id":"../invalid"}),
        ] {
            let mut pod = worker(&spec());
            pod["networkVolume"] = attachment;
            let (provider, requests, task) = server(vec![(200, value()), (200, json!([pod]).to_string())]);
            let mut state = State::Bound { volume: volume() };
            assert!(
                provider
                    .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
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
            assert!(matches!(state, State::Bound { .. }));
        }
    }

    #[test]
    fn attached_or_mismatching_storage_cannot_be_deleted() {
        let mut pod = worker(&spec());
        pod["networkVolume"] = json!({"id":volume().id});
        let (provider, requests, task) = server(vec![(200, value()), (200, json!([pod]).to_string())]);
        let mut state = State::Bound { volume: volume() };
        assert!(
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 2);
        let mut wrong = volume();
        wrong.name = "unrelated".into();
        let (provider, requests, task) = server(vec![(200, serde_json::to_string(&wrong).unwrap())]);
        assert!(
            provider
                .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn worker_request_pins_the_owned_network_volume_and_location() {
        let spec = spec();
        let (provider, requests, task) = server(vec![(200, "[]".into()), (201, worker(&spec).to_string())]);
        provider
            .ensure_with_volume(
                &spec,
                &mut CreateState::Prepared,
                Some(&volume()),
                &Cancellation::default(),
                |_| Ok(()),
                |_| {},
            )
            .unwrap();
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        let body: Value = serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["networkVolumeId"], volume().id);
        assert_eq!(body["dataCenterIds"], json!([volume().data_center_id]));
        assert_eq!(body["volumeInGb"], 0);
        assert_eq!(body["volumeMountPath"], "/workspace");
    }

    #[test]
    fn resource_gate_requires_exact_owned_volume_and_storage_shape() {
        let spec = spec();
        let mut value = worker(&spec);
        value["vcpuCount"] = json!(spec.profile.cpu);
        value["memoryInGb"] = json!(spec.profile.memory_gb);
        value["containerDiskInGb"] = json!(spec.profile.storage.container_gb);
        value["volumeInGb"] = json!(0);
        value["volumeMountPath"] = json!("/workspace");
        value["networkVolume"] = json!({"id":volume().id,"size":volume().size,"dataCenterId":volume().data_center_id});
        let assigned: Worker = serde_json::from_value(value.clone()).unwrap();
        assigned.verify_resources_with_volume(&spec, Some(&volume())).unwrap();
        assert!(assigned.verify_resources(&spec).is_err());
        for field in ["id", "size", "dataCenterId"] {
            let mut invalid = value.clone();
            invalid["networkVolume"][field] = Value::Null;
            assert!(
                serde_json::from_value::<Worker>(invalid)
                    .unwrap()
                    .verify_resources_with_volume(&spec, Some(&volume()))
                    .is_err()
            );
        }
        for (field, wrong) in [
            ("volumeInGb", json!(20)),
            ("volumeMountPath", json!("/other")),
            ("containerDiskInGb", json!(0)),
            ("networkVolume", Value::Null),
        ] {
            let mut invalid = value.clone();
            invalid[field] = wrong;
            assert!(
                serde_json::from_value::<Worker>(invalid)
                    .unwrap()
                    .verify_resources_with_volume(&spec, Some(&volume()))
                    .is_err()
            );
        }
    }

    #[test]
    fn catalog_discovery_honors_configured_placement_and_supported_cpu_capacity() {
        let mut worker = spec();
        worker.data_centers = vec!["preferred".into(), "other".into()];
        let catalog = json!({"dataCenters":[
            {"id":"unconfigured","networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":"HIGH"}]},
            {"id":"other","networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":"HIGH"}]},
            {"id":"preferred","networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":"LOW"}]}
        ]});
        let (mut provider, requests, task) = server(vec![(200, catalog.to_string())]);
        provider.catalog_endpoint = provider.endpoint.clone();
        assert_eq!(
            provider
                .workspace_volume_spec(&worker, &Cancellation::default())
                .unwrap()
                .data_center_id,
            "preferred"
        );
        task.join().unwrap();
        assert!(
            requests.lock().unwrap()[0]
                .starts_with("GET /datacenters?include=CPU_AVAILABILITY&networkVolumeTypes=STANDARD ")
        );
        let (mut provider, _, task) = server(vec![(200, json!({"dataCenters":[{"id":"preferred","networkVolumeTypes":["STANDARD"],"cpuAvailability":[{"id":"cpu3g","availability":"NONE"}]}]}).to_string())]);
        provider.catalog_endpoint = provider.endpoint.clone();
        assert!(
            provider
                .workspace_volume_spec(&worker, &Cancellation::default())
                .is_err()
        );
        task.join().unwrap();
    }
    #[test]
    fn sized_catalog_entries_do_not_certify_flavor_capacity() {
        for id in ["cpu3g-2-4", "cpu3g-4-16", "cpu3g-unknown"] {
            let catalog = json!({"dataCenters":[{
                "id":"available","networkVolumeTypes":["STANDARD"],
                "cpuAvailability":[{"id":id,"availability":"HIGH"}]
            }]});
            let (mut provider, requests, task) = server(vec![(200, catalog.to_string())]);
            provider.catalog_endpoint = provider.endpoint.clone();
            assert!(
                provider
                    .workspace_volume_spec(&spec(), &Cancellation::default())
                    .is_err()
            );
            task.join().unwrap();
            assert_eq!(requests.lock().unwrap().len(), 1);
        }
    }
}
