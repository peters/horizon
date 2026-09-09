use super::super::{
    CloudJobId, CloudWorkflowId, HOST_KEY_BOOTSTRAP_ENV, JOB_ENV, LIFETIME_ENV, PERSISTENT_LIFETIME, PROTOCOL_ENV,
    RunPodApiKey, RunPodClient, RunPodError, RunPodHostKeySource, RunPodInteractiveWorkerProvider, RunPodSshEndpoint,
    RunPodWorker, SSH_PUBLIC_KEY_ENV, WORKFLOW_ENV,
    http::RunPodHttp,
    resource_name,
    tests::{ed25519_key, interactive_request, profile},
};
use super::{Mode, RunPodHostTrust, VERSION, client_digest, sample};
use crate::cloud_run::{
    CloudProvider, WorkerLifetime,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLifetime, InteractiveWorkerProvider,
        InteractiveWorkerRequest, InteractiveWorkerSshEndpoint,
    },
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{self, Cursor, Read},
    sync::{Arc, Mutex},
};

struct Fixture {
    request: InteractiveWorkerRequest,
    worker: RunPodWorker,
    endpoint: RunPodSshEndpoint,
}

impl Fixture {
    fn new() -> Self {
        let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
        request.target.lifetime = WorkerLifetime::Persistent;
        let worker = RunPodWorker {
            workflow_id: request.workflow_id,
            job_id: request.job_id,
            pod_id: "pod_synthetic".into(),
            name: resource_name(request.workflow_id, request.job_id),
            image: request.target.image.clone(),
            lifetime: InteractiveWorkerLifetime::Persistent,
            hourly_cost_micros: None,
        };
        Self {
            request,
            worker,
            endpoint: RunPodSshEndpoint {
                username: "root".into(),
                host: "worker.example".into(),
                port: 2200,
            },
        }
    }

    fn record(&self) -> Value {
        json!({"version": VERSION, "pod_id": self.worker.pod_id, "workflow_id": self.worker.workflow_id,
            "job_id": self.worker.job_id, "cloud_protocol_version": 1,
            "access_digest": client_digest(&self.request.ssh_public_key), "host_public_key": ed25519_key(73)})
    }

    fn pod(&self) -> Value {
        json!({"id": self.worker.pod_id, "name": self.worker.name, "image": self.worker.image, "status": "RUNNING",
            "env": {(WORKFLOW_ENV): self.worker.workflow_id, (JOB_ENV): self.worker.job_id,
                (PROTOCOL_ENV): "1", (SSH_PUBLIC_KEY_ENV): self.request.ssh_public_key,
                (LIFETIME_ENV): PERSISTENT_LIFETIME, (HOST_KEY_BOOTSTRAP_ENV): VERSION.to_string()},
            "ssh": {"direct": {"username": self.endpoint.username, "host": self.endpoint.host, "port": self.endpoint.port}}})
    }

    fn source(&self, responses: Vec<(u16, &'static str, String)>) -> (RunPodHostTrust, Arc<Mutex<Vec<String>>>) {
        let (http, calls) = mock_http(responses);
        (
            RunPodHostTrust {
                expected: self.request.clone(),
                mode: Mode::InitialTaskFree(http),
            },
            calls,
        )
    }

    fn responses(&self, logs: String) -> Vec<(u16, &'static str, String)> {
        vec![
            (200, "application/json", self.pod().to_string()),
            (200, "text/event-stream; charset=utf-8", logs),
            (200, "application/json", self.pod().to_string()),
        ]
    }

    fn key(&self, source: &RunPodHostTrust) -> Option<String> {
        source.host_key(&self.worker, &self.endpoint, &self.request.ssh_public_key)
    }

    fn parse(&self, text: &str) -> Result<Option<String>, RunPodError> {
        sample::parse(
            text.as_bytes(),
            &self.worker,
            &client_digest(&self.request.ssh_public_key),
        )
    }

    fn interactive(&self) -> InteractiveWorker {
        InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: self.worker.workflow_id,
                job_id: self.worker.job_id,
                resource_id: self.worker.pod_id.clone(),
            },
            target: self.request.target.clone(),
            ssh_public_key: self.request.ssh_public_key.clone(),
            lifetime: self.worker.lifetime.clone(),
        }
    }
}

fn mock_http(responses: Vec<(u16, &'static str, String)>) -> (RunPodHttp, Arc<Mutex<Vec<String>>>) {
    let responses = Mutex::new(VecDeque::from(responses));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let config = ureq::Agent::config_builder()
        .middleware(
            move |request: ureq::http::Request<ureq::SendBody>, _next: ureq::middleware::MiddlewareNext| {
                assert_eq!(request.method(), "GET");
                assert_eq!(request.headers()["Authorization"], "Bearer synthetic-credential");
                captured.lock().expect("calls").push(request.uri().to_string());
                let (status, content_type, body) = responses
                    .lock()
                    .expect("responses")
                    .pop_front()
                    .expect("no extra request");
                Ok(ureq::http::Response::builder()
                    .status(status)
                    .header("Content-Type", content_type)
                    .body(ureq::Body::builder().data(body))
                    .expect("mock response"))
            },
        )
        .build();
    (RunPodHttp::mock(ureq::Agent::new_with_config(config)), calls)
}

fn event(line: &str) -> String {
    format!(
        "id: synthetic-cursor\ndata: {}\n\n",
        json!({"source":"container", "line":line, "ts":"2026-09-09T12:00:00Z"})
    )
}

fn record_event(record: &Value) -> String {
    event(&format!("{}{}", sample::PREFIX, record))
}

#[test]
fn initial_sample_binds_expected_identity_between_fresh_reads() {
    let fixture = Fixture::new();
    let logs = format!(
        "{}{}{}",
        event("ordinary startup"),
        record_event(&fixture.record()),
        record_event(&fixture.record())
    );
    let (source, calls) = fixture.source(fixture.responses(logs));
    assert_eq!(fixture.key(&source), Some(ed25519_key(73)));
    assert_eq!(
        *calls.lock().expect("calls"),
        [
            "https://api.runpod.io/v2/pods/pod_synthetic",
            "https://api.runpod.io/v2/pods/pod_synthetic/logs?source=container&tail=5000",
            "https://api.runpod.io/v2/pods/pod_synthetic",
        ]
    );
    assert_eq!(
        client_digest(&fixture.request.ssh_public_key),
        client_digest(&format!("{} client comment", fixture.request.ssh_public_key))
    );
    assert_eq!(format!("{source:?}"), "RunPodHostTrust { .. }");
}

#[test]
fn before_and_after_observations_refuse_identity_or_endpoint_drift() {
    let fixture = Fixture::new();
    for pointer in [
        "/id",
        "/name",
        "/image",
        "/env/HORIZON_WORKFLOW_ID",
        "/env/HORIZON_JOB_ID",
        "/env/HORIZON_CLOUD_PROTOCOL_VERSION",
        "/env/HORIZON_SSH_PUBLIC_KEY",
        "/env/HORIZON_WORKER_LIFETIME",
        "/env/HORIZON_HOST_KEY_BOOTSTRAP_VERSION",
        "/ssh/direct/host",
        "/ssh/direct/username",
        "/ssh/direct/port",
        "/status",
    ] {
        for index in [0, 2] {
            let mut changed = fixture.pod();
            *changed.pointer_mut(pointer).expect("existing binding") = if pointer.ends_with("/port") {
                json!(2201)
            } else {
                json!("drift")
            };
            let mut responses = fixture.responses(record_event(&fixture.record()));
            responses[index].2 = changed.to_string();
            let (source, calls) = fixture.source(responses);
            assert_eq!(fixture.key(&source), None, "{pointer} at read {index}");
            assert_eq!(calls.lock().expect("calls").len(), index + 1);
        }
    }
    let (source, calls) = fixture.source(Vec::new());
    assert_eq!(
        source.host_key(&fixture.worker, &fixture.endpoint, &ed25519_key(99)),
        None
    );
    let mut worker = fixture.worker.clone();
    worker.pod_id = "bad/../pod".into();
    assert_eq!(
        source.host_key(&worker, &fixture.endpoint, &fixture.request.ssh_public_key),
        None
    );
    assert!(calls.lock().expect("calls").is_empty());
}

#[test]
fn malformed_mismatched_and_conflicting_records_never_select_a_key() {
    let fixture = Fixture::new();
    for (field, value) in [
        ("version", json!(VERSION + 1)),
        ("pod_id", json!("other_pod")),
        ("workflow_id", json!(CloudWorkflowId::new())),
        ("job_id", json!(CloudJobId::new())),
        ("cloud_protocol_version", json!(2)),
        ("access_digest", json!("0".repeat(64))),
        ("host_public_key", json!(format!("{} comment", ed25519_key(73)))),
        ("extra", json!("wrong")),
    ] {
        let mut record = fixture.record();
        record[field] = value;
        assert!(fixture.parse(&record_event(&record)).is_err(), "{field}");
    }
    for field in fixture.record().as_object().expect("record").keys() {
        let mut record = fixture.record();
        record.as_object_mut().expect("record").remove(field);
        assert!(fixture.parse(&record_event(&record)).is_err(), "missing {field}");
    }
    let mut other = fixture.record();
    other["host_public_key"] = json!(ed25519_key(74));
    assert!(
        fixture
            .parse(&(record_event(&fixture.record()) + &record_event(&other)))
            .is_err()
    );
    for line in [
        format!(
            "{}{{\"version\":1,{}",
            sample::PREFIX,
            &fixture.record().to_string()[1..]
        ),
        format!("{}not-json", sample::PREFIX),
        format!("{}{}", sample::PREFIX, "x".repeat(1024)),
    ] {
        let error = fixture.parse(&event(&line)).expect_err("invalid bootstrap record");
        assert_eq!(
            error,
            RunPodError::InvalidResponse {
                operation: "host-key bootstrap"
            }
        );
        assert!(!error.to_string().contains(&line));
    }
}

#[test]
fn sse_framing_is_bounded_complete_and_unambiguous() {
    let fixture = Fixture::new();
    let good = record_event(&fixture.record());
    assert_eq!(fixture.parse(&good.replace('\n', "\r\n")), Ok(Some(ed25519_key(73))));
    assert_eq!(fixture.parse(&event("no key here")), Ok(None));
    for invalid in [
        good.trim_end().to_string(),
        good.trim_end_matches('\n').to_string() + "\n",
        good.replace("data:", "data: {}\ndata:"),
        good.replace("id:", "id: duplicate\nid:"),
        good.replace("id:", "event:"),
        good.replace("container", "system"),
        good.replace("2026-09-09T12:00:00Z", "invalid"),
        good.replace("data: {", "data: {\"source\":\"container\","),
        good.replace("data: {", "data: {\"extra\":0,"),
        format!(":{}\n\n", "x".repeat(16 * 1024)),
        "\n".repeat(16_001),
    ] {
        assert!(fixture.parse(&invalid).is_err());
    }
    assert!(sample::parse(&[0xff, b'\n'], &fixture.worker, "digest").is_err());
}

struct EndingReader {
    bytes: Cursor<Vec<u8>>,
    timeout: bool,
}
impl Read for EndingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.read(output)?;
        if count > 0 {
            return Ok(count);
        }
        Err(if self.timeout {
            io::Error::other(ureq::Error::Timeout(ureq::Timeout::Global))
        } else {
            io::Error::other("synthetic private detail")
        })
    }
}

#[test]
fn deadline_ends_only_a_bounded_sample_and_other_read_failures_reject() {
    let fixture = Fixture::new();
    let good = record_event(&fixture.record()).into_bytes();
    let read = sample::read(EndingReader {
        bytes: Cursor::new(good.clone()),
        timeout: true,
    })
    .expect("bounded sample");
    assert_eq!(read, good);
    assert_eq!(
        fixture.parse(std::str::from_utf8(&read).expect("UTF-8")),
        Ok(Some(ed25519_key(73)))
    );
    assert!(
        sample::read(EndingReader {
            bytes: Cursor::new(good),
            timeout: false
        })
        .is_err()
    );
    assert!(sample::read(Cursor::new(vec![b'x'; 2 * 1024 * 1024 + 1])).is_err());
}

#[test]
fn unavailable_or_invalid_logs_have_no_retry_or_fallback() {
    let fixture = Fixture::new();
    for reply in [
        (403, "application/json", String::new()),
        (302, "text/event-stream", String::new()),
        (200, "application/json", record_event(&fixture.record())),
        (200, "text/event-stream", event("no key")),
    ] {
        let (source, calls) = fixture.source(vec![(200, "application/json", fixture.pod().to_string()), reply]);
        assert_eq!(fixture.key(&source), None);
        assert_eq!(calls.lock().expect("calls").len(), 2);
    }
}

#[test]
fn retained_full_pin_needs_only_outer_owned_provider_read_and_no_logs() {
    let fixture = Fixture::new();
    let worker = fixture.interactive();
    let ssh = InteractiveWorkerSshEndpoint {
        host: fixture.endpoint.host.clone(),
        port: fixture.endpoint.port,
        username: fixture.endpoint.username.clone(),
        host_key: ed25519_key(73),
    };
    let source = RunPodHostTrust::retained(&worker, &ssh).expect("retained pin");
    assert_eq!(fixture.key(&source), Some(ssh.host_key.clone()));
    assert_eq!(
        source.host_key(&fixture.worker, &fixture.endpoint, &ed25519_key(99)),
        None
    );
    for field in 0..4 {
        let mut changed_worker = fixture.worker.clone();
        let mut endpoint = fixture.endpoint.clone();
        match field {
            0 => changed_worker.pod_id = "other_pod".into(),
            1 => endpoint.host = "other.example".into(),
            2 => endpoint.port += 1,
            _ => endpoint.username = "other".into(),
        }
        assert_eq!(
            source.host_key(&changed_worker, &endpoint, &fixture.request.ssh_public_key),
            None
        );
    }
    let mut missing = ssh.clone();
    missing.host_key.clear();
    assert!(RunPodHostTrust::retained(&worker, &missing).is_err());
    let mut pod = fixture.pod();
    pod["env"].as_object_mut().expect("env").remove(HOST_KEY_BOOTSTRAP_ENV);
    let (http, calls) = mock_http(vec![(200, "application/json", pod.to_string())]);
    let provider = RunPodInteractiveWorkerProvider::new(RunPodClient::with_transport(http), profile(), source);
    let status = provider.inspect_worker(&worker).expect("inspect").expect("present");
    assert!(status.is_ready_for(&fixture.request, time::OffsetDateTime::now_utc()));
    assert_eq!(status.ssh, Some(ssh));
    assert_eq!(calls.lock().expect("calls").len(), 1);
    let mut invalid = fixture.request;
    invalid.ssh_public_key.clear();
    assert!(RunPodHostTrust::initial_task_free(&RunPodApiKey::new("synthetic-key").expect("key"), &invalid).is_err());
}
