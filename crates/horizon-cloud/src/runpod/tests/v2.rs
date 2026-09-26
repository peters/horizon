use super::*;
use crate::runpod::recovery::Outcome;

#[test]
fn provisioning_and_starting_reconcile_the_same_id_until_ready() {
    for status in ["PROVISIONING", "STARTING"] {
        let spec = spec();
        let mut pod = worker(&spec);
        pod["status"] = json!(status);
        pod["ssh"]["direct"] = Value::Null;
        let (provider, requests, task) = server(vec![(200, pods(&json!([pod]))), (200, pod.to_string())]);
        let mut state = CreateState::Requested;
        let outcome = provider
            .reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(()))
            .unwrap();
        assert!(matches!(outcome.outcome, Outcome::Found { .. }));
        let bound = state.clone();
        let observed = provider
            .ensure(
                &spec,
                &mut state,
                &Cancellation::default(),
                |_| panic!("already bound"),
                |_| {},
            )
            .unwrap();
        assert_eq!(observed.status(), crate::WorkerStatus::Starting);
        assert!(observed.ssh_address().is_none());
        assert_eq!(state, bound);
        task.join().unwrap();
        assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    }
}

#[test]
fn pending_creation_binds_before_resource_readiness() {
    let spec = spec();
    let mut pod = worker(&spec);
    pod["status"] = json!("PROVISIONING");
    pod["ssh"]["direct"] = Value::Null;
    let (provider, _, task) = server(vec![(200, pods(&json!([]))), (201, pod.to_string())]);
    let mut state = CreateState::Prepared;
    let mut transitions = Vec::new();
    let observed = provider
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
    assert!(matches!(state, CreateState::Bound { .. }));
    assert_eq!(transitions.len(), 2);
    assert_eq!(observed.status(), crate::WorkerStatus::Starting);
    assert!(observed.verify_resources(&spec).is_err());
    task.join().unwrap();
}

#[test]
fn only_definite_refusals_allow_the_next_configured_compute_candidate() {
    for (gpu, refusal) in [(false, 400), (false, 403), (true, 400), (true, 403)] {
        let mut spec = spec();
        spec.profile.gpu = gpu;
        spec.cpu_flavors = vec!["cpu3g".into(), "cpu5g".into()];
        spec.gpu_types = vec!["GPU-A".into(), "GPU-B".into()];
        let (provider, requests, task) = server(vec![
            (200, pods(&json!([]))),
            (refusal, json!({"detail":"capacity unavailable"}).to_string()),
            (201, worker(&spec).to_string()),
        ]);
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
        let requests = requests.lock().unwrap();
        let body =
            |index: usize| serde_json::from_str::<Value>(requests[index].split_once("\r\n\r\n").unwrap().1).unwrap();
        let key = if gpu { "gpu" } else { "cpu" };
        assert_eq!(body(1)[key]["id"], if gpu { "GPU-A" } else { "cpu3g" });
        assert_eq!(body(2)[key]["id"], if gpu { "GPU-B" } else { "cpu5g" });
        assert_eq!(
            &transitions[..3],
            &[CreateState::Requested, CreateState::Prepared, CreateState::Requested]
        );
    }
    for response in [
        (401, "{}".into()),
        (402, "{}".into()),
        (413, "{}".into()),
        (422, "{}".into()),
        (429, "{}".into()),
        (500, "{}".into()),
        (503, "{}".into()),
        (201, "{}".into()),
        (201, "not json".into()),
    ] {
        let definitely_rejected = matches!(response.0, 401 | 402 | 413 | 422 | 429);
        let mut spec = spec();
        spec.cpu_flavors.push("cpu5g".into());
        let (provider, requests, task) = server(vec![(200, pods(&json!([]))), response]);
        let mut state = CreateState::Prepared;
        assert!(
            provider
                .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
                .is_err()
        );
        assert_eq!(
            state,
            if definitely_rejected {
                CreateState::Prepared
            } else {
                CreateState::Requested
            }
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
}

#[test]
fn definite_refusal_requires_durable_reset_before_a_later_retry() {
    for status in [402, 413, 429] {
        for failed_reset in [false, true] {
            let spec = spec();
            let mut responses = vec![(200, pods(&json!([]))), (status, "{}".into())];
            if !failed_reset {
                responses.extend([(200, pods(&json!([]))), (201, worker(&spec).to_string())]);
            }
            let (provider, requests, task) = server(responses);
            let mut state = CreateState::Prepared;
            let result = provider.ensure(
                &spec,
                &mut state,
                &Cancellation::default(),
                |next| {
                    if failed_reset && *next == CreateState::Prepared {
                        Err(CloudError::Persistence)
                    } else {
                        Ok(())
                    }
                },
                |_| {},
            );
            if failed_reset {
                assert!(matches!(result, Err(CloudError::Persistence)));
                assert_eq!(state, CreateState::Requested);
            } else {
                assert!(matches!(result, Err(CloudError::Http(code, _)) if code == status));
                assert_eq!(state, CreateState::Prepared);
                provider
                    .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
                    .unwrap();
                assert!(matches!(state, CreateState::Bound { .. }));
            }
            task.join().unwrap();
            assert_eq!(
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|r| r.starts_with("POST "))
                    .count(),
                if failed_reset { 1 } else { 2 }
            );
        }
    }
}

#[test]
fn all_pod_pages_are_required_even_when_the_first_is_empty() {
    let spec = spec();
    let pod = worker(&spec);
    let first = json!({"pods":[],"pagination":{"hasNextPage":true,"nextCursor":"a+/="}}).to_string();
    let (provider, requests, task) = server(vec![(200, first), (200, pods(&json!([pod])))]);
    let mut state = CreateState::Requested;
    let result = provider
        .reconcile(&spec, &mut state, None, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert!(matches!(result.outcome, Outcome::Found { .. }));
    task.join().unwrap();
    assert!(requests.lock().unwrap()[1].starts_with("GET /pods?includeClusterPods=true&cursor=a%2B%2F%3D "));
}

#[test]
fn incomplete_or_cyclic_pages_never_clear_the_creation_fence() {
    for pages in [
        vec![json!([])],
        vec![json!({"pods":[]})],
        vec![json!({"pods":[],"pagination":{"hasNextPage":false}})],
        vec![json!({"pods":[],"pagination":{"hasNextPage":true,"nextCursor":null}})],
        vec![json!({"pods":[],"pagination":{"hasNextPage":false,"nextCursor":"unexpected"}})],
        vec![json!({"pods":[],"pagination":{"hasNextPage":true,"nextCursor":"same"}}); 2],
    ] {
        let (provider, requests, task) = server(pages.into_iter().map(|p| (200, p.to_string())).collect());
        let mut state = CreateState::Requested;
        assert!(
            provider
                .reconcile(&spec(), &mut state, None, &Cancellation::default(), |_| panic!(
                    "cannot bind incomplete results"
                ))
                .is_err()
        );
        assert_eq!(state, CreateState::Requested);
        task.join().unwrap();
        assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    }
}

#[test]
fn v2_wire_mapping_preserves_durable_legacy_format_and_actual_status() {
    let mut pod = worker(&spec());
    pod["cpu"] = json!({"id":"cpu3g","vcpuCount":4,"memory":16});
    pod["mounts"] = json!({"network":[{"volumeId":"volume1","path":"/workspace"}]});
    pod["dataCenterId"] = json!("test-region");
    pod["cost"] = json!(0.16);
    pod["startedAt"] = json!("2026-09-26T00:00:00Z");
    let observed = wire::worker(pod).unwrap();
    assert_eq!(observed.vcpu_count, Some(4));
    assert_eq!(observed.gpu_count, None);
    assert_eq!(observed.memory_in_gb, Some(16));
    assert_eq!(observed.cost_per_hr, Some(0.16));
    assert_eq!(observed.adjusted_cost_per_hr, None);
    assert_eq!(observed.data_center(), Some("test-region"));
    assert_eq!(
        observed.network_volume.as_ref().unwrap().data_center_id.as_deref(),
        Some("test-region")
    );
    assert_eq!(
        observed.network_volume.as_ref().unwrap().size,
        None,
        "mount ID alone never manufactures volume capacity"
    );
    let saved = serde_json::to_value(&observed).unwrap();
    assert_eq!(saved["imageName"], spec().image_digest);
    assert_eq!(saved["desiredStatus"], "RUNNING");
    assert_eq!(saved["lastStartedAt"], "2026-09-26T00:00:00Z");
    assert!(saved.get("status").is_none());
    let reopened: Worker = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(serde_json::to_value(reopened).unwrap(), saved);
    let legacy = saved_worker(&spec());
    let reopened: Worker = serde_json::from_value(legacy).unwrap();
    reopened.verify(&spec()).unwrap();
    assert_eq!(reopened.data_center_id, None);
    assert!(serde_json::to_value(reopened).unwrap().get("dataCenterId").is_none());
}

#[test]
fn v2_gpu_placement_is_retained_without_a_network_volume() {
    let mut spec = spec();
    spec.profile.gpu = true;
    let mut pod = worker(&spec);
    pod["dataCenterId"] = json!("gpu-region");
    let observed = wire::worker(pod).unwrap();
    assert!(observed.network_volume.is_none());
    assert_eq!(observed.data_center(), Some("gpu-region"));
    let saved = serde_json::to_value(&observed).unwrap();
    assert_eq!(saved["dataCenterId"], "gpu-region");
    let reopened: Worker = serde_json::from_value(saved).unwrap();
    assert_eq!(reopened.data_center(), Some("gpu-region"));
}

#[test]
fn legacy_network_volume_placement_remains_available() {
    let mut legacy = saved_worker(&spec());
    legacy["networkVolume"] = json!({"id":"volume1","size":80,"dataCenterId":"legacy-region"});
    let reopened: Worker = serde_json::from_value(legacy).unwrap();
    assert_eq!(reopened.data_center_id, None);
    assert_eq!(reopened.data_center(), Some("legacy-region"));
    assert!(serde_json::to_value(reopened).unwrap().get("dataCenterId").is_none());
}

#[test]
fn omitted_gpu_count_defaults_to_one_but_explicit_invalid_counts_fail() {
    let mut spec = spec();
    spec.profile.gpu = true;
    let mut pod = worker(&spec);
    pod["gpu"] = json!({"id":"GPU-A","vcpuCount":spec.profile.cpu,"memory":spec.profile.memory_gb});
    pod["mounts"] = json!({"persistent":{"size":spec.profile.storage.volume_gb,"path":"/workspace"}});
    let observed = wire::worker(pod.clone()).unwrap();
    assert_eq!(observed.gpu_count, Some(1));
    observed.verify_resources(&spec).unwrap();
    for count in [1, 2] {
        pod["gpu"]["count"] = json!(count);
        assert_eq!(wire::worker(pod.clone()).unwrap().gpu_count, Some(count));
    }
    for count in [json!(0), Value::Null, json!(-1), json!(1.5), json!("1")] {
        pod["gpu"]["count"] = count;
        assert!(matches!(wire::worker(pod.clone()), Err(CloudError::InvalidResponse)));
    }
}

#[test]
fn untrusted_status_ssh_or_mounts_cannot_certify_readiness() {
    let base = worker(&spec());
    let mut cases = Vec::new();
    for (field, value) in [
        ("status", json!("UNKNOWN")),
        ("cost", json!(-1)),
        ("mounts", json!({"network":[{"volumeId":"../bad","path":"/workspace"}]})),
    ] {
        let mut invalid = base.clone();
        invalid[field] = value;
        cases.push(invalid);
    }
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("env");
    cases.push(missing);
    for invalid in cases {
        assert!(matches!(wire::worker(invalid), Err(CloudError::InvalidResponse)));
    }
    for (host, username) in [("worker.example.invalid", "root"), ("192.0.2.1", "custom-user")] {
        let mut unsupported = base.clone();
        unsupported["ssh"] = json!({"direct":{"host":host,"port":22,"username":username}});
        assert!(wire::worker(unsupported).unwrap().ssh_address().is_none());
    }
    let mut proxy = base;
    proxy["ssh"] = json!({"direct":null,"proxy":{"command":"untrusted shell command"}});
    assert!(wire::worker(proxy).unwrap().ssh_address().is_none());
}

#[test]
fn create_body_uses_only_v2_compute_mount_and_credential_fields() {
    let mut spec = spec();
    spec.profile.gpu = true;
    spec.registry_auth_id = Some("pull-generation".into());
    let body = create_body(&spec);
    assert_eq!(body["gpu"]["minVcpuCountPerGpu"], spec.profile.cpu);
    assert_eq!(body["gpu"]["minRamPerGpu"], spec.profile.memory_gb);
    assert_eq!(body["mounts"]["persistent"]["path"], "/workspace");
    assert_eq!(body["registry"], "pull-generation");
    for field in [
        "imageName",
        "computeType",
        "containerRegistryAuthId",
        "gpuTypeIds",
        "volumeInGb",
        "containerDiskInGb",
    ] {
        assert!(body.get(field).is_none());
    }
}
