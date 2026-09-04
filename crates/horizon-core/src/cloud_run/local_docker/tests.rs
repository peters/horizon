use super::*;
use InteractiveWorkerCleanup::{AlreadyAbsent, Deleted};
use LocalDockerError::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct FakeDocker(Arc<Mutex<FakeState>>);
#[derive(Default)]
struct FakeState {
    container: Option<DockerContainer>,
    create_calls: usize,
    delete_calls: usize,
    fail_create_after_insert: bool,
    hidden_inspections_after_create: usize,
    binding_host: Option<String>,
}

impl FakeDocker {
    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DockerTransport for FakeDocker {
    fn inspect(&self, reference: &str) -> DockerResult<Option<DockerContainer>> {
        let mut state = self.state();
        if state.container.is_some() && state.hidden_inspections_after_create > 0 {
            state.hidden_inspections_after_create -= 1;
            return Ok(None);
        }
        let container = state.container.clone();
        Ok(container.filter(|container| container.id == reference || container.name == reference))
    }
    fn inspect_with_timeout(&self, reference: &str, timeout: Duration) -> DockerResult<Option<DockerContainer>> {
        assert!(!timeout.is_zero() && timeout <= CREATE_RECONCILE_TIMEOUT);
        self.inspect(reference)
    }
    fn create(&self, request: &DockerCreateRequest) -> Result<String, LocalDockerError> {
        let mut state = self.state();
        state.create_calls += 1;
        let id = "a".repeat(64);
        state.container = Some(DockerContainer {
            id: id.clone(),
            name: request.name.clone(),
            image: request.image.clone(),
            labels: request.labels.clone(),
            environment: vec![
                format!("{SSH_PUBLIC_KEY_ENV}={}", request.ssh_public_key),
                format!("{TERMINATE_ENV}={}", request.terminate_after),
            ],
            restart_policy: "no".to_string(),
            running: true,
            state: "running".to_string(),
            exit_code: 0,
            ssh_bindings: vec![DockerPortBinding {
                host: state.binding_host.clone().unwrap_or_else(|| LOCAL_HOST.to_string()),
                port: 49_152,
            }],
        });
        if state.fail_create_after_insert {
            return Err(CommandFailed {
                operation: "container creation",
            });
        }
        Ok(id)
    }
    fn read_host_key(&self, _resource_id: &str) -> Result<Option<String>, LocalDockerError> {
        Ok(Some(format!("{} worker@local", ed25519_key(7))))
    }
    fn delete(&self, _resource_id: &str) -> Result<bool, LocalDockerError> {
        let mut state = self.state();
        state.delete_calls += 1;
        state.container = None;
        Ok(true)
    }
}

fn ed25519_key(seed: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn request() -> InteractiveWorkerRequest {
    InteractiveWorkerRequest {
        workflow_id: "00000000-0000-4000-8000-000000000001".parse().expect("workflow ID"),
        job_id: "00000000-0000-4000-8000-000000000002".parse().expect("job ID"),
        target: super::super::WorkerTarget {
            provider: CloudProvider::LocalDocker,
            profile: "local".to_string(),
            image: format!("registry.example/horizon/worker@sha256:{}", "d".repeat(64)),
            disk_gib: 20,
            lease_seconds: 900,
            max_hourly_cost_micros: None,
        },
        ssh_public_key: ed25519_key(3),
    }
}

fn profile(name: &str, docker_host: &str) -> LocalDockerProfile {
    LocalDockerProfile {
        name: name.to_string(),
        docker_host: docker_host.to_string(),
    }
}

fn provider(name: &str, fake: FakeDocker) -> LocalDockerInteractiveWorkerProvider {
    LocalDockerInteractiveWorkerProvider {
        transport: Box::new(fake),
        profile: profile(name, "unix:///var/run/docker.sock"),
    }
}

#[test]
fn creates_reuses_inspects_and_deletes_one_exact_worker() {
    let fake = FakeDocker::default();
    let provider = provider("local", fake.clone());
    let request = request();
    let status = provider.ensure_worker(&request).expect("create").into_status();
    assert!(status.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_eq!(status.ssh.as_ref().expect("SSH").host_key, ed25519_key(7));
    let reused = provider.ensure_worker(&request);
    assert!(matches!(reused, Ok(InteractiveWorkerEnsure::Reused(_))));
    assert_eq!(provider.inspect_worker(&status.worker), Ok(Some(status.clone())));
    assert_eq!(provider.delete_worker(&status.worker), Ok(Deleted));
    assert_eq!(provider.delete_worker(&status.worker), Ok(AlreadyAbsent));
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 1));
}

#[test]
fn rejects_preflight_and_resource_drift_without_unsafe_io() {
    let remote = LocalDockerInteractiveWorkerProvider::new(profile("local", "ssh://remote.example/run/docker.sock"));
    assert_eq!(remote.err(), Some(NonLocalDockerHost));
    let other = provider("other", FakeDocker::default());
    let fake = FakeDocker::default();
    let provider = provider("local", fake.clone());
    let mut invalid = request();
    invalid.target.provider = CloudProvider::RunPod;
    assert_eq!(provider.ensure_worker(&invalid).err(), Some(InvalidTarget));
    assert_eq!(fake.state().create_calls, 0);
    let request = request();
    let worker = provider.ensure_worker(&request).expect("create").into_status().worker;
    assert_eq!(other.inspect_worker(&worker).err(), Some(InvalidPersistedWorker));
    assert_eq!(other.delete_worker(&worker).err(), Some(InvalidPersistedWorker));
    let mut drifted = request;
    drifted.target.disk_gib += 1;
    assert_eq!(provider.ensure_worker(&drifted).err(), Some(ResourceIdentityMismatch));
    let mut worker = worker;
    worker.target.disk_gib += 1;
    assert_eq!(provider.inspect_worker(&worker).err(), Some(ResourceIdentityMismatch));
    assert_eq!(provider.delete_worker(&worker).err(), Some(ResourceIdentityMismatch));
    assert_eq!(fake.state().delete_calls, 0);
}

#[test]
fn reconciles_delayed_lost_create_response_and_cleans_invalid_new_endpoints() {
    let recovered = FakeDocker::default();
    recovered.state().fail_create_after_insert = true;
    recovered.state().hidden_inspections_after_create = 2;
    let reconciled = provider("local", recovered.clone()).ensure_worker(&request());
    assert!(matches!(reconciled, Ok(InteractiveWorkerEnsure::Reused(_))));
    assert_eq!(recovered.state().create_calls, 1);
    let invalid = FakeDocker::default();
    invalid.state().fail_create_after_insert = true;
    invalid.state().binding_host = Some("0.0.0.0".to_string());
    let rejected = provider("local", invalid.clone()).ensure_worker(&request());
    assert_eq!(rejected.err(), Some(InvalidSshEndpoint));
    let state = invalid.state();
    assert_eq!((state.container.is_none(), state.delete_calls), (true, 1));
}
