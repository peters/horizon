use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};
fn spec() -> WorkerSpec {
    WorkerSpec {
        operation_id: "test-operation".into(),
        image_digest: format!("example/worker@sha256:{}", "a".repeat(64)),
        profile: crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["image-only"].clone(),
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".into(),
        registry_auth_id: None,
        gpu_types: vec!["NVIDIA RTX A4000".into()],
        cpu_flavors: vec!["cpu3g".into()],
        data_centers: vec![],
    }
}
fn worker(spec: &WorkerSpec) -> Value {
    json!({"id":"worker1","name":spec.name(),"imageName":spec.image_digest,"desiredStatus":"RUNNING","publicIp":"192.0.2.1","portMappings":{"22":22001}})
}
#[test]
fn power_actions_verify_identity_and_accept_empty_success_bodies() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, worker(&spec).to_string()),
        (200, String::new()),
        (200, worker(&spec).to_string()),
        (200, String::new()),
    ]);
    provider.stop(&spec, "worker1", &Cancellation::default()).unwrap();
    provider.start(&spec, "worker1", &Cancellation::default()).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("POST /pods/worker1/stop "));
    assert!(requests[3].starts_with("POST /pods/worker1/start "));
    let mut wrong = worker(&spec);
    wrong["name"] = json!("unrelated-worker");
    let (provider, requests, task) = server(vec![(200, wrong.to_string())]);
    assert!(matches!(
        provider.start(&spec, "worker1", &Cancellation::default()),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}
#[test]
fn stopped_worker_without_an_address_does_not_break_account_reconciliation() {
    let mut value = worker(&spec());
    value["desiredStatus"] = json!("EXITED");
    value["publicIp"] = json!("");
    value["portMappings"] = Value::Null;
    let (provider, _, task) = server(vec![(200, json!([value]).to_string())]);
    let workers = provider.list(&Cancellation::default()).unwrap();
    assert_eq!(workers[0].status(), crate::WorkerStatus::Stopped);
    assert!(workers[0].ssh_address().is_none());
    task.join().unwrap();
}
fn server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    delayed_server(responses, None)
}
fn delayed_server(
    responses: Vec<(u16, String)>,
    delay: Option<(usize, Duration)>,
) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let observed = requests.clone();
    let task = thread::spawn(move || {
        for (index, (status, body)) in responses.into_iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut input = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                input.extend_from_slice(&buf[..n]);
                if let Some(end) = input.windows(4).position(|x| x == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&input[..end]);
                    let length = header
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if input.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            observed.lock().unwrap().push(String::from_utf8(input).unwrap());
            if let Some((delayed_index, duration)) = delay
                && index == delayed_index
            {
                thread::sleep(duration);
                // The client has timed out; the provider still created the worker.
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            } else {
                write!(stream,"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        }
    });
    let mut provider = RunPod::new(Credential::new("secret-test-key".into()).unwrap());
    provider.endpoint = format!("http://{addr}");
    (provider, requests, task)
}

#[test]
fn timed_out_post_reconciles_created_worker_without_repeating_allocation() {
    let spec = spec();
    let created = worker(&spec);
    let (mut provider, requests, task) = delayed_server(
        vec![
            (200, "[]".into()),
            (201, created.to_string()),
            (200, json!([created]).to_string()),
        ],
        Some((1, Duration::from_millis(600))),
    );
    provider.agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(200)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build(),
    );
    let mut state = CreateState::Prepared;
    let mut persisted = Vec::new();
    assert!(
        provider
            .ensure(
                &spec,
                &mut state,
                &Cancellation::default(),
                |next| {
                    persisted.push(next.clone());
                    Ok(())
                },
                |_| {}
            )
            .is_err()
    );
    assert_eq!(state, CreateState::Requested);
    assert_eq!(persisted, vec![CreateState::Requested]);
    thread::sleep(Duration::from_millis(650));
    let found = provider
        .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    assert_eq!(found.id, "worker1");
    assert_eq!(
        state,
        CreateState::Bound {
            worker_id: "worker1".into()
        }
    );
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}
#[test]
fn create_persists_fence_before_post_and_reconnect_never_allocates() {
    let spec = spec();
    let response = worker(&spec).to_string();
    let (provider, requests, task) = server(vec![(200, "[]".into()), (201, response.clone()), (200, response)]);
    let mut state = CreateState::Prepared;
    let mut saved = vec![];
    let first = provider
        .ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |s| {
                saved.push(s.clone());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    assert_eq!(saved[0], CreateState::Requested);
    let second = provider
        .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    assert_eq!(first.id, second.id);
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
fn uncertain_post_and_empty_reconciliation_never_retry() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (503, "SECRET BODY".into()),
        (200, "[]".into()),
    ]);
    let mut state = CreateState::Prepared;
    let err = provider
        .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap_err();
    assert!(!err.to_string().contains("SECRET"));
    assert_eq!(state, CreateState::Requested);
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::CreationUnresolved)
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
fn persistence_failure_prevents_allocation_and_duplicates_fail_closed() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (200, json!([worker(&spec), worker(&spec)]).to_string()),
    ]);
    let mut state = CreateState::Prepared;
    assert!(matches!(
        provider.ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |_| Err(CloudError::Persistence),
            |_| {}
        ),
        Err(CloudError::Persistence)
    ));
    assert_eq!(state, CreateState::Prepared);
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::DuplicateWorkers)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}
#[test]
fn cancellation_and_removed_worker_do_not_create() {
    let spec = spec();
    let cancel = Cancellation::default();
    cancel.cancel();
    let (provider, _, task) = server(vec![(404, "{}".into())]);
    let mut state = CreateState::Bound {
        worker_id: "worker1".into(),
    };
    assert!(matches!(
        provider.ensure(&spec, &mut state, &cancel, |_| Ok(()), |_| {}),
        Err(CloudError::Cancelled)
    ));
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::WorkerLost)
    ));
    task.join().unwrap();
}
#[test]
fn cpu_gpu_specs_and_credential_debug() {
    let mut spec = spec();
    assert_eq!(create_body(&spec)["computeType"], "CPU");
    spec.profile.gpu = true;
    assert_eq!(create_body(&spec)["minRAMPerGPU"], 8);
    assert!(!format!("{:?}", Credential::new("secret-value".into()).unwrap()).contains("secret-value"));
}

#[test]
fn definite_refusal_can_retry_but_no_post_after_pre_send_cancel() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (400, "refused".into()),
        (200, "[]".into()),
        (201, worker(&spec).to_string()),
    ]);
    let mut state = CreateState::Prepared;
    assert!(matches!(
        provider.ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {}),
        Err(CloudError::Rejected)
    ));
    assert_eq!(state, CreateState::Prepared);
    assert!(
        provider
            .ensure(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
            .is_ok()
    );
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST"))
            .count(),
        2
    );
    let (provider, requests, task) = server(vec![(200, "[]".into())]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Prepared;
    assert!(matches!(
        provider.ensure(
            &spec,
            &mut state,
            &cancel,
            |_| Ok(()),
            |p| if p == Progress::Requesting {
                cancel.cancel();
            }
        ),
        Err(CloudError::Cancelled)
    ));
    assert_eq!(state, CreateState::Prepared);
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}
#[test]
fn documented_response_alias_and_empty_stop_response() {
    let spec = spec();
    let mut response = worker(&spec);
    response["image"] = response["imageName"].take();
    response.as_object_mut().unwrap().remove("imageName");
    response["costPerHr"] = json!("0.74");
    let (provider, _, task) = server(vec![(200, response.to_string()), (200, String::new())]);
    provider.stop(&spec, "worker1", &Cancellation::default()).unwrap();
    task.join().unwrap();
}
#[test]
fn termination_checks_identity_then_proves_absence() {
    let spec = spec();
    let (provider, requests, task) = server(vec![
        (200, worker(&spec).to_string()),
        (204, String::new()),
        (404, "{}".into()),
    ]);
    let mut state = CreateState::Bound {
        worker_id: "worker1".into(),
    };
    provider
        .terminate(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert!(matches!(state, CreateState::Terminated { .. }));
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 3);
}
#[test]
fn unsuitable_cpu_memory_fails_before_provider_io() {
    let mut spec = spec();
    spec.profile.memory_gb = 32;
    spec.cpu_flavors = vec!["cpu3g".into()];
    assert!(spec.validate().is_err());
    spec.cpu_flavors = vec!["cpu3m".into()];
    assert!(spec.validate().is_ok());
}

#[test]
fn readiness_requires_reported_resources_and_a_gpu_when_requested() {
    let mut spec = spec();
    let mut value = worker(&spec);
    let parse = |value: &Value| serde_json::from_value::<Worker>(value.clone()).unwrap();
    assert!(parse(&value).verify_resources(&spec).is_err());
    value["vcpuCount"] = json!(spec.profile.cpu);
    value["memoryInGb"] = json!(spec.profile.memory_gb);
    assert!(parse(&value).verify_resources(&spec).is_ok());
    spec.profile.gpu = true;
    assert!(parse(&value).verify_resources(&spec).is_err());
    value["gpuCount"] = json!(0);
    assert!(parse(&value).verify_resources(&spec).is_err());
    value["gpuCount"] = json!(1);
    assert!(parse(&value).verify_resources(&spec).is_ok());
    value["memoryInGb"] = json!(0);
    assert!(parse(&value).verify_resources(&spec).is_err());
}

#[test]
fn malformed_public_keys_are_rejected_before_provider_access() {
    let valid = spec().public_key;
    for key in [
        "ssh-ed25519 ",
        "ssh-ed25519 invalid",
        "ssh-ed25519 AAAA",
        "ssh-rsa AAAA",
    ] {
        let mut spec = spec();
        spec.public_key = key.into();
        assert!(matches!(
            spec.validate(),
            Err(CloudError::Invalid("Worker requires an Ed25519 public key"))
        ));
    }
    let mut spec = spec();
    spec.public_key = format!("{valid} fixture@example.invalid");
    spec.validate().unwrap();
    spec.public_key.push('\n');
    assert!(spec.validate().is_err());
}
