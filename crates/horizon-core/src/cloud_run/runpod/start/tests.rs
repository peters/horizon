use super::super::{
    CreatePodRequest, RunPodApiKey, RunPodCleanup, RunPodClient, RunPodHostTrust, RunPodNetworkVolumeExpectation,
    Transport,
    http::RunPodHttp,
    network_volume::ApiNetworkVolume,
    resource_name,
    tests::{ed25519_key, interactive_request, profile},
};
use super::*;
use crate::cloud_run::interactive_worker::{InteractiveWorkerIdentity, InteractiveWorkerLifecycle};
use crate::cloud_run::{CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Fake(Arc<Mutex<State>>);
struct State {
    pod: Option<ApiPod>,
    gets: VecDeque<Result<Option<ApiPod>, RunPodError>>,
    volumes: VecDeque<Option<Value>>,
    calls: Vec<&'static str>,
    start_error: Option<RunPodError>,
}
impl Transport for Fake {
    fn network_volume(&self, id: &str) -> Result<Option<ApiNetworkVolume>, RunPodError> {
        assert_eq!(id, "volume_exact");
        let mut state = self.0.lock().expect("state");
        state.calls.push("volume");
        let value = state.volumes.pop_front().unwrap_or_else(|| Some(volume()));
        Ok(value.map(|value| serde_json::from_value(value).expect("volume")))
    }
    fn list_by_name(&self, _: &str) -> Result<Vec<ApiPod>, RunPodError> {
        panic!("no list")
    }
    fn create(&self, _: &CreatePodRequest) -> Result<ApiPod, RunPodError> {
        panic!("no create")
    }
    fn delete(&self, _: &str) -> Result<RunPodCleanup, RunPodError> {
        panic!("no delete")
    }
    fn stop(&self, _: &str) -> Result<(), RunPodError> {
        panic!("no stop")
    }
    fn get(&self, id: &str) -> Result<Option<ApiPod>, RunPodError> {
        assert_eq!(id, "pod_exact");
        let mut state = self.0.lock().expect("state");
        state.calls.push("get");
        state.gets.pop_front().unwrap_or_else(|| Ok(state.pod.clone()))
    }
    fn start(&self, id: &str) -> Result<(), RunPodError> {
        assert_eq!(id, "pod_exact");
        let mut state = self.0.lock().expect("state");
        state.calls.push("start");
        if let Some(pod) = &mut state.pod {
            pod.status = Some("RUNNING".into());
            pod.stop.runtime = Some(json!({"uptime":0}));
        }
        state.start_error.take().map_or(Ok(()), Err)
    }
}

fn volume() -> Value {
    json!({"id":"volume_exact","dataCenter":"EUR-NO-1","size":20,"type":"HIGH_PERFORMANCE"})
}
fn pin() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: "worker.example".into(),
        port: 2200,
        username: "root".into(),
        host_key: ed25519_key(73),
    }
}
struct Fixture {
    worker: InteractiveWorker,
    raw: Value,
    fake: Fake,
    network: bool,
}
impl Fixture {
    fn new(network: bool) -> Self {
        let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
        request.target.lifetime = WorkerLifetime::Persistent;
        let raw = json!({
            "id":"pod_exact", "name":resource_name(request.workflow_id, request.job_id),
            "image":request.target.image, "status":"EXITED", "cost":0,
            "env":{"HORIZON_WORKFLOW_ID":request.workflow_id,"HORIZON_JOB_ID":request.job_id,
                "HORIZON_CLOUD_PROTOCOL_VERSION":"1","HORIZON_WORKER_LIFETIME":"persistent",
                "HORIZON_SSH_PUBLIC_KEY":request.ssh_public_key},
            "ssh":{"direct":{"host":"worker.example","port":2200,"username":"root"}},
            "mounts": if network { json!({"network":[{"volumeId":"volume_exact","path":"/workspace"}]}) }
                      else { json!({"persistent":{"size":20,"path":"/workspace"}}) },
            "cloud":"SECURE","dataCenterId":"EUR-NO-1","cluster":null,
            "locked":false,"actions":["start","terminate"],"runtime":null
        });
        let worker = InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "pod_exact".into(),
            },
            target: request.target,
            ssh_public_key: request.ssh_public_key,
            lifetime: InteractiveWorkerLifetime::Persistent,
        };
        Self {
            worker,
            fake: Fake(Arc::new(Mutex::new(State {
                pod: Some(serde_json::from_value(raw.clone()).expect("pod")),
                gets: VecDeque::new(),
                volumes: VecDeque::new(),
                calls: Vec::new(),
                start_error: None,
            }))),
            raw,
            network,
        }
    }
    fn provider(&self) -> RunPodInteractiveWorkerProvider {
        let client = RunPodClient::with_transport_and_fence(
            self.fake.clone(),
            |_, _, _: &crate::cloud_run::WorkerTarget, _: &str| panic!("no claim"),
        );
        let trust = RunPodHostTrust::retained(&self.worker, &pin()).expect("retained trust");
        if self.network {
            let request = crate::cloud_run::interactive_worker::InteractiveWorkerRequest {
                workflow_id: self.worker.identity.workflow_id,
                job_id: self.worker.identity.job_id,
                target: self.worker.target.clone(),
                ssh_public_key: self.worker.ssh_public_key.clone(),
            };
            RunPodInteractiveWorkerProvider::new_with_network_volume(
                client,
                profile(),
                trust,
                &request,
                &RunPodNetworkVolumeExpectation {
                    volume_id: "volume_exact".into(),
                    data_center_id: "EUR-NO-1".into(),
                    minimum_size_gb: 10,
                },
            )
            .expect("network")
        } else {
            RunPodInteractiveWorkerProvider::new(client, profile(), trust)
        }
    }
    fn set(&self, raw: Value) {
        self.fake.0.lock().expect("state").pod = Some(serde_json::from_value(raw).expect("pod"));
    }
    fn calls(&self) -> Vec<&'static str> {
        self.fake.0.lock().expect("state").calls.clone()
    }
    fn queued(&self, values: Vec<Option<Value>>) {
        self.fake.0.lock().expect("state").gets = values
            .into_iter()
            .map(|raw| Ok(raw.map(|raw| serde_json::from_value(raw).expect("pod"))))
            .collect();
    }
}

#[test]
fn starts_once_then_reopened_running_worker_is_read_only_with_original_pin() {
    for network in [false, true] {
        let f = Fixture::new(network);
        let provider = f.provider();
        let before = f.worker.clone();
        let InteractiveWorkerStart::Started(status) = provider.start_worker(&f.worker).expect("start") else {
            panic!("started")
        };
        assert_eq!(status.worker, before);
        assert_eq!(status.lifecycle, InteractiveWorkerLifecycle::Ready);
        assert_eq!(status.ssh, Some(pin()));
        assert_eq!(
            f.provider().start_worker(&f.worker),
            Ok(InteractiveWorkerStart::AlreadyRunning(status))
        );
        assert_eq!(
            f.calls(),
            if network {
                vec!["get", "volume", "start", "get", "volume", "get", "volume"]
            } else {
                vec!["get", "start", "get", "get"]
            }
        );
        assert_eq!(f.worker, before);
    }
}

#[test]
fn invalid_saved_worker_profile_and_nonretained_trust_refuse_before_io() {
    for changed in 0..7 {
        let f = Fixture::new(false);
        let mut provider = f.provider();
        let mut worker = f.worker.clone();
        match changed {
            0 => worker.identity.resource_id = "../other".into(),
            1 => worker.identity.provider = CloudProvider::Azure,
            2 => worker.ssh_public_key.clear(),
            3 => worker.target.profile = "other".into(),
            4 => provider.profile.volume_gib = 0,
            5 => {
                let request = interactive_request(worker.identity.workflow_id, worker.identity.job_id);
                provider.host_keys = Box::new(
                    RunPodHostTrust::initial_task_free(&RunPodApiKey::new("synthetic").expect("key"), &request)
                        .expect("initial"),
                );
            }
            _ => {
                worker.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3600 };
                worker.lifetime = InteractiveWorkerLifetime::TimeLimited(
                    crate::cloud_run::interactive_worker::InteractiveWorkerLease {
                        terminate_after: super::super::termination_deadline(3600).expect("deadline"),
                    },
                );
            }
        }
        assert!(provider.start_worker(&worker).is_err());
        assert!(f.calls().is_empty());
    }
}

#[test]
fn exact_absence_never_creates_and_post_dispatch_absence_is_unverified() {
    let f = Fixture::new(false);
    f.fake.0.lock().expect("state").pod = None;
    assert_eq!(
        f.provider().start_worker(&f.worker),
        Ok(InteractiveWorkerStart::AlreadyAbsent)
    );
    assert_eq!(f.calls(), vec!["get"]);
    let f = Fixture::new(false);
    f.queued(vec![Some(f.raw.clone()), None]);
    assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
    assert_eq!(f.calls(), vec!["get", "start", "get"]);
}

#[test]
fn stopped_state_requires_positive_runtime_capability_and_storage_proof() {
    for changed in 0..14 {
        let f = Fixture::new(false);
        let mut raw = f.raw.clone();
        match changed {
            0 => {
                raw.as_object_mut().expect("pod").remove("runtime");
            }
            1 => raw["runtime"] = json!({"uptime":1}),
            2 => raw["actions"] = json!(["terminate"]),
            3 => raw["locked"] = json!(true),
            4 => raw["mounts"] = json!({}),
            5 => raw["mounts"]["persistent"]["size"] = json!(19),
            6 => raw["mounts"]["persistent"]["path"] = json!("/other"),
            7 => raw["mounts"]["network"] = json!([]),
            8 => raw["cloud"] = json!("COMMUNITY"),
            9 => raw["cluster"] = json!({"id":"foreign"}),
            10 => raw["status"] = json!("TERMINATED"),
            11 => raw["status"] = json!("ERROR"),
            12 => raw["status"] = json!("PROVISIONING"),
            _ => raw["status"] = json!("unknown"),
        }
        f.set(raw);
        assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
        assert_eq!(f.calls(), vec!["get"]);
    }
}

#[test]
fn ownership_ssh_and_retained_mount_drift_on_either_side_never_succeeds() {
    for after in [false, true] {
        for changed in 0..10 {
            let f = Fixture::new(false);
            let mut raw = f.raw.clone();
            match changed {
                0 => raw["id"] = json!("foreign"),
                1 => raw["name"] = json!("foreign"),
                2 => raw["image"] = json!("other"),
                3 => raw["env"]["HORIZON_WORKFLOW_ID"] = json!(CloudWorkflowId::new()),
                4 => raw["env"]["HORIZON_JOB_ID"] = json!(CloudJobId::new()),
                5 => raw["env"]["HORIZON_SSH_PUBLIC_KEY"] = json!(ed25519_key(2)),
                6 => raw["env"]["HORIZON_TERMINATE_AFTER"] = json!("2099-01-01T00:00:00Z"),
                7 => raw["ssh"]["direct"]["host"] = json!("replacement.example"),
                8 => raw["ssh"]["direct"]["port"] = json!(2222),
                _ => raw["mounts"]["persistent"]["size"] = json!(21),
            }
            if changed == 9 && !after {
                continue;
            } // A larger initial retained volume is allowed.
            f.queued(if after {
                vec![Some(f.raw.clone()), Some(raw)]
            } else {
                vec![Some(raw)]
            });
            assert!(f.provider().start_worker(&f.worker).is_err());
            assert_eq!(
                f.calls(),
                if after {
                    vec!["get", "start", "get"]
                } else {
                    vec!["get"]
                }
            );
        }
    }
}

#[test]
fn ambiguous_start_is_observed_without_retry_and_pending_observation_is_bounded() {
    for outcome in 0..3 {
        let error = || match outcome {
            0 => None,
            1 => Some(RunPodError::RequestFailed { operation: "pod Start" }),
            _ => Some(RunPodError::InvalidResponse { operation: "pod Start" }),
        };
        let f = Fixture::new(false);
        f.fake.0.lock().expect("state").start_error = error();
        assert!(matches!(
            f.provider().start_worker(&f.worker),
            Ok(InteractiveWorkerStart::Started(_))
        ));
        assert_eq!(f.calls(), vec!["get", "start", "get"]);
        let f = Fixture::new(false);
        f.fake.0.lock().expect("state").start_error = error();
        f.queued(vec![Some(f.raw.clone()); 4]);
        assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
        assert_eq!(f.calls(), vec!["get", "start", "get", "get", "get"]);
    }
    let f = Fixture::new(false);
    let mut starting = f.raw.clone();
    starting["status"] = json!("STARTING");
    let mut running = starting.clone();
    running["status"] = json!("RUNNING");
    running["runtime"] = json!({"uptime":0});
    f.queued(vec![Some(starting.clone()), Some(starting), Some(running)]);
    assert!(matches!(
        f.provider().start_worker(&f.worker),
        Ok(InteractiveWorkerStart::Started(_))
    ));
    assert_eq!(f.calls(), vec!["get", "get", "get"]);
}

#[test]
fn missing_endpoint_stays_provisioning_and_inconsistent_running_is_rejected() {
    let f = Fixture::new(false);
    let mut raw = f.raw.clone();
    raw["status"] = json!("RUNNING");
    raw["runtime"] = json!({"uptime":0});
    raw["ssh"] = json!({});
    f.set(raw.clone());
    let result = f.provider().start_worker(&f.worker).expect("running");
    assert_eq!(
        result.status().expect("status").lifecycle,
        InteractiveWorkerLifecycle::Provisioning
    );
    assert!(result.status().expect("status").ssh.is_none());
    assert_eq!(f.calls(), vec!["get"]);
    raw["runtime"] = Value::Null;
    f.set(raw);
    assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
}

#[test]
fn selected_volume_is_checked_before_start_and_during_every_observation() {
    for after in [false, true] {
        for changed in 0..4 {
            let f = Fixture::new(true);
            let mut value = volume();
            match changed {
                0 => value["id"] = json!("foreign"),
                1 => value["type"] = json!("STANDARD"),
                2 => value["dataCenter"] = json!("other"),
                _ => value["size"] = json!(9),
            }
            f.fake.0.lock().expect("state").volumes = if after {
                vec![Some(volume()), Some(value)]
            } else {
                vec![Some(value)]
            }
            .into();
            assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
            assert_eq!(
                f.calls(),
                if after {
                    vec!["get", "volume", "start", "get", "volume"]
                } else {
                    vec!["get", "volume"]
                }
            );
        }
    }
}

#[test]
fn network_attachment_drift_and_failed_observations_do_not_retry_start() {
    for after in [false, true] {
        for changed in 0..4 {
            let f = Fixture::new(true);
            let mut raw = f.raw.clone();
            match changed {
                0 => raw["mounts"]["network"][0]["volumeId"] = json!("other"),
                1 => raw["mounts"]["network"][0]["path"] = json!("/other"),
                2 => raw["dataCenterId"] = json!("other"),
                _ => raw["mounts"]["persistent"] = json!({"size":20,"path":"/workspace"}),
            }
            f.queued(if after {
                vec![Some(f.raw.clone()), Some(raw)]
            } else {
                vec![Some(raw)]
            });
            assert_eq!(f.provider().start_worker(&f.worker), Err(RunPodError::StartUnverified));
            assert_eq!(
                f.calls().iter().filter(|call| **call == "start").count(),
                usize::from(after)
            );
        }
    }
    let f = Fixture::new(false);
    f.fake.0.lock().expect("state").gets = VecDeque::from([
        Ok(Some(serde_json::from_value(f.raw.clone()).expect("pod"))),
        Err(RunPodError::RequestFailed {
            operation: "pod inspection",
        }),
    ]);
    assert_eq!(
        f.provider().start_worker(&f.worker),
        Err(RunPodError::RequestFailed {
            operation: "pod inspection"
        })
    );
    assert_eq!(f.calls(), vec!["get", "start", "get"]);
}

#[test]
fn http_start_uses_exact_v2_action_once_and_redacts_invalid_responses() {
    use std::{
        io::Read as _,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use ureq::{
        Body, SendBody,
        http::{Request, Response},
        middleware::MiddlewareNext,
    };
    for (code, body) in [
        (200, r#"{"id":"pod_exact"}"#.to_string()),
        (200, r#"{"id":"foreign"}"#.into()),
        (200, "private malformed response".into()),
        (200, "x".repeat(2 * 1024 * 1024 + 1)),
        (403, "private denied".into()),
        (404, "private absent".into()),
        (409, "private conflict".into()),
        (302, "private redirect".into()),
        (204, String::new()),
    ] {
        let should_succeed = code == 200 && body == r#"{"id":"pod_exact"}"#;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let agent = ureq::Agent::config_builder()
            .middleware(move |request: Request<SendBody>, _: MiddlewareNext| {
                count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.method(), "POST");
                assert_eq!(request.uri(), "https://api.runpod.io/v2/pods/pod_exact/action");
                assert_eq!(request.headers()["Authorization"], "Bearer synthetic-credential");
                let mut payload = String::new();
                request
                    .into_body()
                    .into_reader()
                    .read_to_string(&mut payload)
                    .expect("body");
                assert_eq!(
                    serde_json::from_str::<Value>(&payload).expect("json"),
                    json!({"action":"start"})
                );
                Ok(Response::builder()
                    .status(code)
                    .body(Body::builder().data(body.clone()))
                    .expect("response"))
            })
            .build()
            .new_agent();
        let result = RunPodHttp::mock(agent).start("pod_exact");
        assert_eq!(result.is_ok(), should_succeed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!format!("{result:?}").contains("private"));
        assert!(!format!("{result:?}").contains("synthetic-credential"));
    }
    let http = RunPodHttp::new(&RunPodApiKey::new("synthetic").expect("key"));
    for id in ["", "../foreign", "pod/action", "pod?start=true"] {
        assert_eq!(http.start(id), Err(RunPodError::ResourceIdentityMismatch));
    }
}
