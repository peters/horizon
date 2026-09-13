//! GET-only retained coordinates. Authentication and persistence belong to the caller.

use super::{RunPodInteractiveWorkerProvider, runpod_worker};
use crate::cloud_run::{
    ArtifactDigest,
    interactive_worker::{InteractiveWorker, InteractiveWorkerLifetime, InteractiveWorkerSshEndpoint},
    interactive_worker_start::{InteractiveWorkerEndpointCandidate, InteractiveWorkerEndpointObserver},
    runpod::{RunPodError, RunPodLifecycle, status_from_resource, validate_target},
};

impl InteractiveWorkerEndpointObserver for RunPodInteractiveWorkerProvider {
    fn observe_endpoint_candidate(
        &self,
        worker: &InteractiveWorker,
        saved: &InteractiveWorkerSshEndpoint,
    ) -> Result<InteractiveWorkerEndpointCandidate, RunPodError> {
        self.client.check_network_worker(worker)?;
        let retained = runpod_worker(worker)?;
        validate_target(&worker.target, &self.profile)?;
        if worker.lifetime != InteractiveWorkerLifetime::Persistent
            || !saved.is_complete()
            || self.host_keys.retained_endpoint(worker).as_ref() != Some(saved)
        {
            return Err(RunPodError::StartIdentityRequired);
        }
        if self.client.network_binding.is_none() && self.profile.volume_gib == 0 {
            return Err(RunPodError::StopRetentionUnverified);
        }
        let pod = self
            .client
            .transport
            .get(&retained.pod_id)?
            .ok_or(RunPodError::StartUnverified)?;
        let status = status_from_resource(&pod, &retained, Some(&worker.ssh_public_key))?;
        if status.lifecycle != RunPodLifecycle::Running
            || !pod.stop.runtime.as_ref().is_some_and(serde_json::Value::is_object)
        {
            return Err(RunPodError::StartUnverified);
        }
        self.client.retained_mount(&pod, &self.profile)?;
        let direct = pod
            .ssh
            .as_ref()
            .and_then(|ssh| ssh.direct.as_ref())
            .ok_or(RunPodError::StartUnverified)?;
        let mut candidate_pin = saved.clone();
        candidate_pin.host.clone_from(&direct.host);
        candidate_pin.port = direct.port;
        if direct.username != saved.username || !candidate_pin.is_complete() {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        let mounts = serde_json::to_vec(pod.stop.mounts()).map_err(|_| RunPodError::StopRetentionUnverified)?;
        Ok(InteractiveWorkerEndpointCandidate {
            worker: worker.clone(),
            host: direct.host.clone(),
            port: direct.port,
            username: direct.username.clone(),
            storage_fingerprint: ArtifactDigest::sha256(&mounts),
            network_volume: self
                .client
                .network_binding
                .as_ref()
                .map(|binding| binding.selection.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_run::{
        CloudJobId, CloudWorkflowId, WorkerLifetime,
        interactive_worker::InteractiveWorkerIdentity,
        runpod::{
            ApiPod, CreatePodRequest, RunPodCleanup, RunPodClient, RunPodHostTrust, RunPodNetworkVolumeExpectation,
            Transport,
            network_volume::ApiNetworkVolume,
            resource_name,
            tests::{ed25519_key, interactive_request, profile},
        },
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake(Arc<Mutex<State>>);
    struct State {
        pod: Option<Value>,
        volume: Option<Value>,
        calls: Vec<&'static str>,
    }
    impl Transport for Fake {
        fn get(&self, _: &str) -> Result<Option<ApiPod>, RunPodError> {
            let mut state = self.0.lock().expect("state");
            state.calls.push("get");
            Ok(state.pod.clone().map(|raw| serde_json::from_value(raw).expect("pod")))
        }
        fn network_volume(&self, _: &str) -> Result<Option<ApiNetworkVolume>, RunPodError> {
            let mut state = self.0.lock().expect("state");
            state.calls.push("volume");
            Ok(state
                .volume
                .clone()
                .map(|raw| serde_json::from_value(raw).expect("volume")))
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
        fn start(&self, _: &str) -> Result<(), RunPodError> {
            panic!("no start")
        }
        fn stop(&self, _: &str) -> Result<(), RunPodError> {
            panic!("no stop")
        }
    }

    struct Fixture {
        fake: Fake,
        worker: InteractiveWorker,
        saved: InteractiveWorkerSshEndpoint,
        selection: RunPodNetworkVolumeExpectation,
    }
    impl Fixture {
        fn new(network: bool) -> Self {
            let mut request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
            request.target.lifetime = WorkerLifetime::Persistent;
            let raw = json!({"id":"pod_exact", "name":resource_name(request.workflow_id,request.job_id),
                "image":request.target.image,"status":"RUNNING","cost":0,
                "env":{"HORIZON_WORKFLOW_ID":request.workflow_id,"HORIZON_JOB_ID":request.job_id,
                "HORIZON_CLOUD_PROTOCOL_VERSION":"1","HORIZON_WORKER_LIFETIME":"persistent",
                "HORIZON_SSH_PUBLIC_KEY":request.ssh_public_key},
                "ssh":{"direct":{"host":"new.example","port":2300,"username":"root"}},
                "mounts":if network {json!({"network":[{"volumeId":"volume_exact","path":"/workspace"}]})}
                    else {json!({"persistent":{"size":20,"path":"/workspace"}})},
                "cloud":"SECURE","cluster":null,"dataCenterId":"EUR-NO-1","runtime":{"uptime":1}});
            let worker = InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: crate::cloud_run::CloudProvider::RunPod,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "pod_exact".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            };
            Self {
                fake: Fake(Arc::new(Mutex::new(State {
                    pod: Some(raw),
                    volume: Some(json!({"id":"volume_exact",
                "dataCenter":"EUR-NO-1","size":20,"type":"HIGH_PERFORMANCE"})),
                    calls: vec![],
                }))),
                worker,
                saved: InteractiveWorkerSshEndpoint {
                    host: "old.example".into(),
                    port: 2200,
                    username: "root".into(),
                    host_key: ed25519_key(73),
                },
                selection: RunPodNetworkVolumeExpectation {
                    volume_id: "volume_exact".into(),
                    data_center_id: "EUR-NO-1".into(),
                    minimum_size_gb: 20,
                },
            }
        }
        fn provider(&self, network: bool) -> RunPodInteractiveWorkerProvider {
            let client = RunPodClient::with_transport_and_fence(
                self.fake.clone(),
                |_, _, _: &crate::cloud_run::WorkerTarget, _: &str| panic!("no claim"),
            );
            let trust = RunPodHostTrust::retained(&self.worker, &self.saved).expect("trust");
            if network {
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
                    &self.selection,
                )
                .expect("bound")
            } else {
                RunPodInteractiveWorkerProvider::new(client, profile(), trust)
            }
        }
    }

    #[test]
    fn changed_coordinates_are_only_untrusted_get_results_with_retained_storage() {
        for network in [false, true] {
            let fixture = Fixture::new(network);
            let candidate = fixture
                .provider(network)
                .observe_endpoint_candidate(&fixture.worker, &fixture.saved)
                .expect("candidate");
            assert_eq!(candidate.worker, fixture.worker);
            assert_eq!(
                (candidate.host.as_str(), candidate.port, candidate.username.as_str()),
                ("new.example", 2300, "root")
            );
            assert_eq!(candidate.network_volume, network.then_some(fixture.selection));
            assert_eq!(
                fixture.fake.0.lock().expect("state").calls,
                if network { vec!["get", "volume"] } else { vec!["get"] }
            );
        }
    }

    #[test]
    fn incomplete_or_different_retained_pin_refuses_before_io() {
        let fixture = Fixture::new(false);
        for field in 0..4 {
            let mut changed = fixture.saved.clone();
            match field {
                0 => changed.port += 1,
                1 => changed.host = "elsewhere.example".into(),
                2 => changed.host_key = ed25519_key(74),
                _ => changed.username = "other".into(),
            }
            assert!(
                fixture
                    .provider(false)
                    .observe_endpoint_candidate(&fixture.worker, &changed)
                    .is_err()
            );
        }
        let client = RunPodClient::with_transport_and_fence(
            fixture.fake.clone(),
            |_, _, _: &crate::cloud_run::WorkerTarget, _: &str| panic!("no claim"),
        );
        let provider = RunPodInteractiveWorkerProvider::new(
            client,
            profile(),
            |_: &crate::cloud_run::runpod::RunPodWorker, _: &crate::cloud_run::runpod::RunPodSshEndpoint, _: &str| {
                panic!("no bootstrap")
            },
        );
        assert!(
            provider
                .observe_endpoint_candidate(&fixture.worker, &fixture.saved)
                .is_err()
        );
        assert!(fixture.fake.0.lock().expect("state").calls.is_empty());
    }

    #[test]
    fn absent_unready_foreign_and_unretained_snapshots_never_produce_coordinates() {
        for field in 0..11 {
            let fixture = Fixture::new(false);
            {
                let mut state = fixture.fake.0.lock().expect("state");
                let raw = state.pod.as_mut().expect("pod");
                match field {
                    0 => {
                        state.pod = None;
                    }
                    1 => raw["id"] = json!("other"),
                    2 => raw["status"] = json!("STARTING"),
                    3 => raw["runtime"] = Value::Null,
                    4 => {
                        raw.as_object_mut().expect("object").remove("runtime");
                    }
                    5 => raw["env"]["HORIZON_SSH_PUBLIC_KEY"] = json!(ed25519_key(74)),
                    6 => raw["mounts"]["persistent"]["path"] = json!("/tmp"),
                    7 => raw["cloud"] = json!("COMMUNITY"),
                    8 => raw["ssh"]["direct"]["username"] = json!("other"),
                    9 => raw["ssh"]["direct"]["port"] = json!(0),
                    _ => raw["image"] = json!("other/worker:latest"),
                }
            }
            assert!(
                fixture
                    .provider(false)
                    .observe_endpoint_candidate(&fixture.worker, &fixture.saved)
                    .is_err(),
                "case {field}"
            );
            assert_eq!(fixture.fake.0.lock().expect("state").calls, ["get"]);
        }
        let fixture = Fixture::new(true);
        fixture.fake.0.lock().expect("state").volume = None;
        assert!(
            fixture
                .provider(true)
                .observe_endpoint_candidate(&fixture.worker, &fixture.saved)
                .is_err()
        );
        assert_eq!(fixture.fake.0.lock().expect("state").calls, ["get", "volume"]);
    }
}
