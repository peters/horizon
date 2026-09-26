use super::*;

fn catalog_server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let (mut provider, requests, task) = server(responses);
    provider.catalog_endpoint.clone_from(&provider.endpoint);
    (provider, requests, task)
}
fn gpu_spec(floor: Option<&str>) -> WorkerSpec {
    let mut spec = spec();
    spec.profile.gpu = true;
    spec.profile.min_cuda_version = floor.map(str::to_owned);
    spec.gpu_types = vec!["NVIDIA RTX A4000".into(), "NVIDIA L4".into()];
    spec
}
fn gpu(id: &str, versions: &[&str]) -> Value {
    gpu_with(id, &versions.iter().map(|version| (*version, true)).collect::<Vec<_>>())
}
fn gpu_with(id: &str, versions: &[(&str, bool)]) -> Value {
    let versions: Vec<Value> = versions
        .iter()
        .map(|(version, available)| json!({"version": version, "available": available}))
        .collect();
    json!({"id": id, "name": id, "memory": 24, "secure": true, "price": {"secure": 0.5}, "cudaVersions": versions})
}
fn gpu_in(id: &str, version: &str, centers: &[(&str, &str)]) -> Value {
    let mut gpu = gpu(id, &[version]);
    gpu["dataCenters"] = centers
        .iter()
        .map(|(center, availability)| json!({"id": center, "name": center, "availability": availability}))
        .collect();
    gpu
}
fn ensure(provider: &RunPod, spec: &WorkerSpec, state: &mut CreateState) -> Result<Worker, CloudError> {
    provider.ensure(spec, state, &Cancellation::default(), |_| Ok(()), |_| {})
}
fn body(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn a_cuda_floor_requests_every_available_version_the_pod_create_accepts() {
    let spec = gpu_spec(Some("12.8"));
    let catalog = json!({"gpus": [
        // The catalog already applies the floor; an older entry is still never sent, nor
        // is 12.9, which is offered but full right now.
        gpu_with(
            "NVIDIA RTX A4000",
            &[("12.8", true), ("12.4", true), ("13.1", true), ("12.9", false)]
        ),
        gpu("NVIDIA L4", &["13.0", "12.10", "12.8"]),
        gpu("NVIDIA H100 80GB HBM3", &["12.9"]),
        json!({"id": "AMD Instinct MI300X OAM", "name": "MI300X", "memory": 192, "secure": true, "price": {"secure": 2.5}})
    ]});
    let (provider, requests, task) = catalog_server(vec![
        (200, "[]".into()),
        (200, catalog.to_string()),
        (201, worker(&spec).to_string()),
    ]);
    let mut state = CreateState::Prepared;
    ensure(&provider, &spec, &mut state).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(
        requests[1].starts_with("GET /gpus?include=AVAILABILITY&product=POD&cloud=SECURE&minCudaVersion=12.8 "),
        "{}",
        requests[1]
    );
    assert!(requests[2].starts_with("POST /pods "));
    // Newest first and deduplicated. 13.1 and 12.10 are not in the v1 create schema, and
    // the H100 is not a requested GPU type.
    assert_eq!(body(&requests[2])["allowedCudaVersions"], json!(["13.0", "12.8"]));
    assert_eq!(body(&requests[2])["gpuTypeIds"], json!(spec.gpu_types));
}

#[test]
fn a_floor_no_requested_gpu_meets_fails_before_any_allocation() {
    let spec = gpu_spec(Some("13.0"));
    let catalog = json!({"gpus": [
        {"id": "NVIDIA RTX A4000", "name": "RTX A4000", "memory": 16, "secure": true, "price": {"secure": 0.25}, "cudaVersions": []},
        gpu_with("NVIDIA L4", &[("13.0", false)]),
        gpu("NVIDIA H100 80GB HBM3", &["13.0"])
    ]});
    let (provider, requests, task) = catalog_server(vec![(200, "[]".into()), (200, catalog.to_string())]);
    let mut state = CreateState::Prepared;
    let mut saved = Vec::new();
    let error = provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |state| {
                saved.push(state.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap_err();
    task.join().unwrap();
    assert!(matches!(&error, CloudError::CudaUnavailable(floor) if floor == "13.0"));
    assert!(error.to_string().contains("min_cuda_version"), "{error}");
    assert_eq!(state, CreateState::Prepared);
    assert!(saved.is_empty(), "no creation fence without a request");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn without_a_floor_the_create_sends_no_cuda_constraint_and_skips_the_catalog() {
    for spec in [gpu_spec(None), {
        // A floor saved on a CPU profile never reaches a request.
        let mut spec = spec();
        spec.profile.min_cuda_version = Some("12.8".into());
        spec
    }] {
        let responses = if spec.profile.gpu {
            vec![(200, "[]".into()), (201, worker(&spec).to_string())]
        } else {
            Vec::new()
        };
        let (provider, requests, task) = catalog_server(responses);
        let mut state = CreateState::Prepared;
        let result = ensure(&provider, &spec, &mut state);
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        if spec.profile.gpu {
            result.unwrap();
            assert!(requests[1].starts_with("POST /pods "));
            assert!(body(&requests[1]).get("allowedCudaVersions").is_none());
        } else {
            assert!(result.is_err(), "a CPU profile with a floor is invalid");
            assert!(requests.is_empty());
        }
    }
}

#[test]
fn a_malformed_or_failed_catalog_never_creates() {
    for (status, catalog) in [
        (200, json!({"gpus": [gpu("NVIDIA L4", &["twelve"])]}).to_string()),
        (200, json!({"gpu": []}).to_string()),
        (500, String::new()),
    ] {
        let spec = gpu_spec(Some("12.8"));
        let (provider, requests, task) = catalog_server(vec![(200, "[]".into()), (status, catalog)]);
        let mut state = CreateState::Prepared;
        assert!(ensure(&provider, &spec, &mut state).is_err());
        task.join().unwrap();
        assert_eq!(state, CreateState::Prepared);
        assert!(!requests.lock().unwrap().iter().any(|r| r.starts_with("POST ")));
    }
}

#[test]
fn a_version_free_only_outside_the_workers_data_centers_is_not_requested() {
    let mut spec = gpu_spec(Some("12.8"));
    spec.data_centers = vec!["EU-RO-1".into(), "EU-SE-1".into()];
    let floor = json!({"gpus": [gpu("NVIDIA L4", &["13.0", "12.9", "12.8"])]});
    // 13.0 is free in another data center but full here; 12.9 is free here only on a
    // GPU type the worker did not request; 12.8 is free here.
    let exact_13_0 = json!({"gpus": [gpu_in("NVIDIA L4", "13.0", &[("US-TX-3", "HIGH"), ("EU-RO-1", "NONE")])]});
    let exact_12_9 = json!({"gpus": [
        gpu_in("NVIDIA L4", "12.9", &[("US-TX-3", "LOW")]),
        gpu_in("NVIDIA H100 80GB HBM3", "12.9", &[("EU-SE-1", "HIGH")])
    ]});
    let exact_12_8 = json!({"gpus": [gpu_in("NVIDIA L4", "12.8", &[("EU-SE-1", "LOW")])]});
    let (provider, requests, task) = catalog_server(vec![
        (200, "[]".into()),
        (200, floor.to_string()),
        (200, exact_13_0.to_string()),
        (200, exact_12_9.to_string()),
        (200, exact_12_8.to_string()),
        (201, worker(&spec).to_string()),
    ]);
    let mut state = CreateState::Prepared;
    ensure(&provider, &spec, &mut state).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    for (request, version) in requests[2..5].iter().zip(["13.0", "12.9", "12.8"]) {
        let expected = format!("GET /gpus?include=AVAILABILITY&product=POD&cloud=SECURE&cudaVersions={version} ");
        assert!(request.starts_with(&expected), "{request}");
    }
    let body = body(&requests[5]);
    assert_eq!(body["allowedCudaVersions"], json!(["12.8"]));
    assert_eq!(body["dataCenterIds"], json!(spec.data_centers));
}

#[test]
fn a_floor_met_only_outside_the_workers_data_centers_fails_before_any_allocation() {
    let mut spec = gpu_spec(Some("13.0"));
    spec.data_centers = vec!["EU-RO-1".into()];
    let floor = json!({"gpus": [gpu("NVIDIA L4", &["13.0"])]});
    for exact in [
        json!({"gpus": [gpu_in("NVIDIA L4", "13.0", &[("US-TX-3", "HIGH"), ("EU-RO-1", "NONE")])]}),
        // No data center has the type free on this version, so the list is omitted.
        json!({"gpus": [gpu("NVIDIA L4", &["13.0"])]}),
    ] {
        let (provider, requests, task) = catalog_server(vec![
            (200, "[]".into()),
            (200, floor.to_string()),
            (200, exact.to_string()),
        ]);
        let mut state = CreateState::Prepared;
        let error = ensure(&provider, &spec, &mut state).unwrap_err();
        task.join().unwrap();
        assert!(matches!(&error, CloudError::CudaUnavailable(floor) if floor == "13.0"));
        assert!(error.to_string().contains("data centers"), "{error}");
        assert_eq!(state, CreateState::Prepared);
        assert!(!requests.lock().unwrap().iter().any(|r| r.starts_with("POST ")));
    }
}
