use super::*;

fn gpu_spec(floor: Option<&str>) -> WorkerSpec {
    let mut spec = spec();
    spec.profile.gpu = true;
    spec.profile.min_cuda_version = floor.map(str::to_owned);
    spec.gpu_types = vec!["NVIDIA RTX A4000".into(), "NVIDIA L4".into()];
    spec.data_centers = vec!["EU-RO-1".into()];
    spec
}
fn ensure(provider: &RunPod, spec: &WorkerSpec, state: &mut CreateState) -> Result<Worker, CloudError> {
    provider.ensure(spec, state, &Cancellation::default(), |_| Ok(()), |_| {})
}
fn body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn v2_create_sends_the_cuda_floor_without_a_v1_version_allowlist() {
    for floor in ["12.8", "12.11", "13.1"] {
        let spec = gpu_spec(Some(floor));
        let (provider, requests, task) = server(vec![(200, pods(&json!([]))), (201, worker(&spec).to_string())]);
        ensure(&provider, &spec, &mut CreateState::Prepared).unwrap();
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "no v1 catalog expansion");
        assert!(requests[1].starts_with("POST /pods "));
        let body = body(&requests[1]);
        assert_eq!(body["gpu"]["minCudaVersion"], floor);
        assert_eq!(body["gpu"]["id"], spec.gpu_types[0]);
        assert_eq!(body["dataCenterIds"], json!(spec.data_centers));
        assert!(body.get("allowedCudaVersions").is_none());
        assert!(body["gpu"].get("allowedCudaVersions").is_none());
    }
}

#[test]
fn cuda_floor_survives_each_definite_capacity_fallback() {
    let spec = gpu_spec(Some("13.0"));
    let (provider, requests, task) = server(vec![
        (200, pods(&json!([]))),
        (400, json!({"detail":"capacity unavailable"}).to_string()),
        (201, worker(&spec).to_string()),
    ]);
    ensure(&provider, &spec, &mut CreateState::Prepared).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    for (request, id) in requests[1..].iter().zip(&spec.gpu_types) {
        let body = body(request);
        assert_eq!(body["gpu"]["id"], *id);
        assert_eq!(body["gpu"]["minCudaVersion"], "13.0");
        assert_eq!(body["dataCenterIds"], json!(spec.data_centers));
    }
}

#[test]
fn uncertain_cuda_placement_keeps_the_creation_fence() {
    let spec = gpu_spec(Some("13.0"));
    let (provider, requests, task) = server(vec![(200, pods(&json!([]))), (500, "{}".into())]);
    let mut state = CreateState::Prepared;
    assert!(ensure(&provider, &spec, &mut state).is_err());
    task.join().unwrap();
    assert_eq!(state, CreateState::Requested);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[test]
fn without_a_cuda_floor_the_gpu_request_has_no_cuda_constraint() {
    let spec = gpu_spec(None);
    let (provider, requests, task) = server(vec![(200, pods(&json!([]))), (201, worker(&spec).to_string())]);
    ensure(&provider, &spec, &mut CreateState::Prepared).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    let body = body(&requests[1]);
    assert!(body["gpu"].get("minCudaVersion").is_none());
    assert!(body["gpu"].get("allowedCudaVersions").is_none());
}

#[test]
fn invalid_cuda_floors_fail_before_any_provider_request() {
    let mut invalid = vec![gpu_spec(Some("12")), gpu_spec(Some("twelve"))];
    let mut cpu = spec();
    cpu.profile.gpu = false;
    cpu.profile.min_cuda_version = Some("12.8".into());
    invalid.push(cpu);
    for spec in invalid {
        let (provider, requests, task) = server(Vec::new());
        let mut state = CreateState::Prepared;
        assert!(ensure(&provider, &spec, &mut state).is_err());
        task.join().unwrap();
        assert_eq!(state, CreateState::Prepared);
        assert!(requests.lock().unwrap().is_empty());
    }
}
