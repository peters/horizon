use super::*;
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
    binding_host: Option<String>,
}

impl FakeDocker {
    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DockerTransport for FakeDocker {
    fn inspect(&self, reference: &str) -> Result<Option<DockerContainer>, LocalDockerError> {
        let container = self.state().container.clone();
        Ok(container.filter(|container| container.id == reference || container.name == reference))
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
            return Err(LocalDockerError::CommandFailed {
                operation: "container creation",
            });
        }
        Ok(id)
    }

    fn read_host_key(&self, _resource_id: &str) -> Result<Option<String>, LocalDockerError> {
        Ok(Some(ed25519_key(7, Some("worker@local"))))
    }

    fn delete(&self, _resource_id: &str) -> Result<bool, LocalDockerError> {
        let mut state = self.state();
        state.delete_calls += 1;
        state.container = None;
        Ok(true)
    }
}

fn ed25519_key(seed: u8, comment: Option<&str>) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([seed; 32]);
    let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
    comment.map_or(key.clone(), |comment| format!("{key} {comment}"))
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
        ssh_public_key: ed25519_key(3, Some("client@local")),
    }
}

fn provider(fake: FakeDocker) -> LocalDockerInteractiveWorkerProvider {
    let profile = LocalDockerProfile {
        name: "local".to_string(),
        docker_host: "unix:///var/run/docker.sock".to_string(),
    };
    LocalDockerInteractiveWorkerProvider::with_transport(profile, fake)
}

#[test]
fn creates_reuses_inspects_and_deletes_one_exact_worker() {
    let fake = FakeDocker::default();
    let provider = provider(fake.clone());
    let request = request();
    let InteractiveWorkerEnsure::Created(status) = provider.ensure_worker(&request).expect("create") else {
        panic!("expected a created worker");
    };
    assert!(status.is_ready_for(&request, time::OffsetDateTime::now_utc()));
    assert_eq!(status.ssh.as_ref().expect("SSH").host_key, ed25519_key(7, None));
    assert!(matches!(
        provider.ensure_worker(&request),
        Ok(InteractiveWorkerEnsure::Reused(_))
    ));
    assert_eq!(provider.inspect_worker(&status.worker), Ok(Some(status.clone())));
    let deleted = provider.delete_worker(&status.worker);
    let absent = provider.delete_worker(&status.worker);
    assert!(matches!(deleted, Ok(InteractiveWorkerCleanup::Deleted)));
    assert!(matches!(absent, Ok(InteractiveWorkerCleanup::AlreadyAbsent)));
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 1));
}

#[test]
fn rejects_preflight_and_resource_drift_without_unsafe_io() {
    assert!(matches!(
        LocalDockerInteractiveWorkerProvider::new(LocalDockerProfile {
            name: "local".to_string(),
            docker_host: "ssh://remote.example/run/docker.sock".to_string(),
        }),
        Err(LocalDockerError::NonLocalDockerHost)
    ));
    let fake = FakeDocker::default();
    let provider = provider(fake.clone());
    let mut invalid = request();
    invalid.target.provider = CloudProvider::RunPod;
    assert_eq!(provider.ensure_worker(&invalid), Err(LocalDockerError::InvalidTarget));
    assert_eq!(fake.state().create_calls, 0);

    let request = request();
    let worker = provider.ensure_worker(&request).expect("create").into_status().worker;
    let mut drifted = request;
    drifted.target.disk_gib += 1;
    assert_eq!(
        provider.ensure_worker(&drifted),
        Err(LocalDockerError::ResourceIdentityMismatch)
    );
    let mut worker = worker;
    worker.target.disk_gib += 1;
    assert_eq!(
        provider.inspect_worker(&worker),
        Err(LocalDockerError::ResourceIdentityMismatch)
    );
    assert_eq!(
        provider.delete_worker(&worker),
        Err(LocalDockerError::ResourceIdentityMismatch)
    );
    assert_eq!(fake.state().delete_calls, 0);
}

#[test]
fn reconciles_a_lost_create_response_and_cleans_invalid_new_endpoints() {
    let recovered = FakeDocker::default();
    recovered.state().fail_create_after_insert = true;
    assert!(matches!(
        provider(recovered.clone()).ensure_worker(&request()),
        Ok(InteractiveWorkerEnsure::Reused(_))
    ));
    assert_eq!(recovered.state().create_calls, 1);

    let invalid = FakeDocker::default();
    invalid.state().binding_host = Some("0.0.0.0".to_string());
    assert_eq!(
        provider(invalid.clone()).ensure_worker(&request()),
        Err(LocalDockerError::InvalidSshEndpoint)
    );
    let state = invalid.state();
    assert!(state.container.is_none());
    assert_eq!(state.delete_calls, 1);
}
