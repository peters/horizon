//! Local Docker implementation of persistent and time-limited interactive workers.

use super::{
    CLOUD_RUN_PROTOCOL_VERSION, CloudProvider, WorkerLifetime, WorkerTarget,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
        InteractiveWorkerLease, InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider,
        InteractiveWorkerRequest, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus, valid_ssh_public_key,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use thiserror::Error;

mod command;
#[cfg(test)]
mod tests;

const MANAGED_LABEL: &str = "io.horizon.local-docker";
const PROVIDER_LABEL: &str = "io.horizon.provider";
const PROTOCOL_LABEL: &str = "io.horizon.cloud-protocol";
const WORKFLOW_LABEL: &str = "io.horizon.workflow";
const JOB_LABEL: &str = "io.horizon.job";
const TARGET_LABEL: &str = "io.horizon.target";
const SSH_KEY_LABEL: &str = "io.horizon.ssh-public-key";
const TERMINATE_LABEL: &str = "io.horizon.terminate-after";
const SSH_PUBLIC_KEY_ENV: &str = "HORIZON_SSH_PUBLIC_KEY";
const TERMINATE_ENV: &str = "HORIZON_TERMINATE_AFTER";
const LOCAL_HOST: &str = "127.0.0.1";
const SSH_USERNAME: &str = "root";
const HOST_KEY_PATH: &str = "/etc/ssh/ssh_host_ed25519_key.pub";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const CREATE_RECONCILE_TIMEOUT: Duration = Duration::from_secs(2);
const CREATE_RECONCILE_POLL_INTERVAL: Duration = Duration::from_millis(50);
type DockerResult<T> = Result<T, LocalDockerError>;

/// Non-secret configuration for the local Docker daemon provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDockerProfile {
    pub name: String,
    pub docker_host: String,
}

/// Interactive worker provider backed by an explicitly selected local Docker daemon.
pub struct LocalDockerInteractiveWorkerProvider {
    transport: Box<dyn DockerTransport>,
    profile: LocalDockerProfile,
}

impl LocalDockerInteractiveWorkerProvider {
    /// Build a provider pinned to an explicit local Unix socket or Windows named pipe.
    ///
    /// # Errors
    /// Returns [`LocalDockerError::NonLocalDockerHost`] for ambient or remote daemon endpoints.
    pub fn new(profile: LocalDockerProfile) -> Result<Self, LocalDockerError> {
        valid_local_docker_host(&profile.docker_host)
            .then(|| Self {
                transport: Box::new(command::DockerCli::new(&profile.docker_host)),
                profile,
            })
            .ok_or(LocalDockerError::NonLocalDockerHost)
    }
    fn ensure_existing(
        &self,
        request: &InteractiveWorkerRequest,
        container: &DockerContainer,
    ) -> DockerResult<InteractiveWorkerStatus> {
        let worker = worker_for_request(container, request)?;
        if !worker
            .lifetime
            .is_valid_at(request.target.lifetime, time::OffsetDateTime::now_utc())
        {
            return self.reject_unbounded_lease(&worker, container);
        }
        self.observe(container, worker)
    }
    fn validate_persisted_worker(&self, worker: &InteractiveWorker) -> DockerResult<()> {
        validate_worker(worker)?;
        (worker.target.profile == self.profile.name)
            .then_some(())
            .ok_or(LocalDockerError::InvalidPersistedWorker)
    }
    fn reject_unbounded_lease(
        &self,
        worker: &InteractiveWorker,
        container: &DockerContainer,
    ) -> DockerResult<InteractiveWorkerStatus> {
        let resource_id = worker.identity.resource_id.clone();
        if verify_container_for_worker(container, worker).is_err()
            || self.transport.delete(&resource_id).is_err()
            || !matches!(self.transport.inspect(&resource_id), Ok(None))
        {
            return Err(LocalDockerError::LeaseRejectionCleanupFailed { resource_id });
        }
        Err(LocalDockerError::LeaseDeadlineRejected { resource_id })
    }
    fn create_worker(
        &self,
        request: &InteractiveWorkerRequest,
        container_name: &str,
    ) -> DockerResult<InteractiveWorkerEnsure> {
        let create = DockerCreateRequest::new(request, container_name)?;
        match self.transport.create(&create) {
            Ok(resource_id) if valid_container_id(&resource_id) => {
                let container = match self.transport.inspect(&resource_id) {
                    Ok(Some(container)) => container,
                    Ok(None) => {
                        return self.reconcile_uncertain_create(
                            request,
                            &create,
                            invalid_response("container creation verification"),
                        );
                    }
                    Err(error) => return self.reconcile_uncertain_create(request, &create, error),
                };
                if container.id != resource_id {
                    let error = LocalDockerError::ResourceIdentityMismatch;
                    return self.reconcile_uncertain_create(request, &create, error);
                }
                let worker = match worker_for_request(&container, request) {
                    Ok(worker) => worker,
                    Err(error) => return self.cleanup_invalid_creation(&container, &create, error),
                };
                if worker
                    .lifetime
                    .as_time_limited()
                    .map(|lease| lease.terminate_after.as_str())
                    != create.terminate_after.as_deref()
                {
                    return self.cleanup_invalid_creation(
                        &container,
                        &create,
                        LocalDockerError::ResourceIdentityMismatch,
                    );
                }
                match self.observe(&container, worker) {
                    Ok(status) => Ok(InteractiveWorkerEnsure::Created(status)),
                    Err(error @ LocalDockerError::ResourceAbsent) => {
                        self.reconcile_uncertain_create(request, &create, error)
                    }
                    Err(error) => self.cleanup_invalid_creation(&container, &create, error),
                }
            }
            Ok(_) => self.reconcile_uncertain_create(request, &create, invalid_response("container creation")),
            Err(error) => self.reconcile_uncertain_create(request, &create, error),
        }
    }
    fn reconcile_uncertain_create(
        &self,
        request: &InteractiveWorkerRequest,
        create: &DockerCreateRequest,
        original: LocalDockerError,
    ) -> DockerResult<InteractiveWorkerEnsure> {
        let deadline = Instant::now() + CREATE_RECONCILE_TIMEOUT;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Ok(Some(container)) = self.transport.inspect_with_timeout(&create.name, remaining) {
                match self.ensure_existing(request, &container) {
                    Ok(status) => return Ok(InteractiveWorkerEnsure::Reused(status)),
                    Err(LocalDockerError::ResourceAbsent) => {}
                    Err(error) if create.terminate_after.is_none() => {
                        return Err(persistent_reconciliation_error(&container, create, error));
                    }
                    Err(error) => return self.cleanup_invalid_creation(&container, create, error),
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(CREATE_RECONCILE_POLL_INTERVAL.min(remaining));
        }
        Err(original)
    }
    fn cleanup_invalid_creation(
        &self,
        container: &DockerContainer,
        create: &DockerCreateRequest,
        original: LocalDockerError,
    ) -> DockerResult<InteractiveWorkerEnsure> {
        let resource_id = container.id.clone();
        if !created_container_matches(container, create) {
            if create.terminate_after.is_none()
                && created_container_identity_matches(container, create)
                && !lifetime_matches(container, None)
            {
                return Err(LocalDockerError::PersistentLifetimeMetadataConflict { resource_id });
            }
            return Err(original);
        }
        if self.transport.delete(&resource_id).is_err() || !matches!(self.transport.inspect(&resource_id), Ok(None)) {
            return Err(LocalDockerError::CreationCleanupFailed { resource_id });
        }
        Err(original)
    }
    fn observe(&self, container: &DockerContainer, worker: InteractiveWorker) -> DockerResult<InteractiveWorkerStatus> {
        let (lifecycle, ssh) = match (container.state.as_str(), container.running) {
            ("running", true) => self.running_connection(container)?,
            ("created" | "restarting", false) => (InteractiveWorkerLifecycle::Provisioning, None),
            ("exited" | "dead", false) if container.exit_code == 0 => (InteractiveWorkerLifecycle::Stopped, None),
            ("exited" | "dead", false) => (InteractiveWorkerLifecycle::Failed, None),
            ("removing", false) => (InteractiveWorkerLifecycle::Deleting, None),
            _ => (InteractiveWorkerLifecycle::Unknown, None),
        };
        Ok(InteractiveWorkerStatus { worker, lifecycle, ssh })
    }
    fn running_connection(
        &self,
        container: &DockerContainer,
    ) -> DockerResult<(InteractiveWorkerLifecycle, Option<InteractiveWorkerSshEndpoint>)> {
        let Some(binding) = container.ssh_bindings.as_slice().first() else {
            return Ok((InteractiveWorkerLifecycle::Provisioning, None));
        };
        if container.ssh_bindings.len() != 1 || binding.host != LOCAL_HOST || binding.port == 0 {
            return Err(LocalDockerError::InvalidSshEndpoint);
        }
        let Some(raw_host_key) = self.transport.read_host_key(&container.id)? else {
            return Ok((InteractiveWorkerLifecycle::Provisioning, None));
        };
        let host_key = canonical_ed25519_key(&raw_host_key).ok_or(LocalDockerError::InvalidHostKey)?;
        let endpoint = InteractiveWorkerSshEndpoint {
            host: LOCAL_HOST.to_string(),
            port: binding.port,
            username: SSH_USERNAME.to_string(),
            host_key,
        };
        Ok((InteractiveWorkerLifecycle::Ready, Some(endpoint)))
    }
}

impl InteractiveWorkerProvider for LocalDockerInteractiveWorkerProvider {
    type Error = LocalDockerError;
    fn provider(&self) -> CloudProvider {
        CloudProvider::LocalDocker
    }
    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        validate_request(request, &self.profile)?;
        let container_name = container_name(request.workflow_id, request.job_id);
        if let Some(container) = self.transport.inspect(&container_name)? {
            match self.ensure_existing(request, &container) {
                Err(LocalDockerError::ResourceAbsent) => {}
                result => return result.map(InteractiveWorkerEnsure::Reused),
            }
        }
        self.create_worker(request, &container_name)
    }
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.validate_persisted_worker(worker)?;
        let Some(container) = self.transport.inspect(&worker.identity.resource_id)? else {
            return Ok(None);
        };
        verify_container_for_worker(&container, worker)?;
        match self.observe(&container, worker.clone()) {
            Err(LocalDockerError::ResourceAbsent) => Ok(None),
            result => result.map(Some),
        }
    }
    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.validate_persisted_worker(worker)?;
        let resource_id = &worker.identity.resource_id;
        let Some(container) = self.transport.inspect(resource_id)? else {
            return Ok(InteractiveWorkerCleanup::AlreadyAbsent);
        };
        verify_container_for_worker(&container, worker)?;
        let _ = self.transport.delete(resource_id)?;
        if self.transport.inspect(resource_id)?.is_some() {
            return Err(LocalDockerError::DeletionVerificationFailed {
                resource_id: resource_id.clone(),
            });
        }
        Ok(InteractiveWorkerCleanup::Deleted)
    }
}
#[derive(Debug, Error, Eq, PartialEq)]
pub enum LocalDockerError {
    #[error("local Docker worker target or profile is invalid")]
    InvalidTarget,
    #[error("persisted local Docker worker identity is invalid")]
    InvalidPersistedWorker,
    #[error("local Docker provider requires an explicit local Unix socket or Windows named pipe")]
    NonLocalDockerHost,
    #[error("required command 'docker' is unavailable")]
    DockerUnavailable,
    #[error("local Docker command exceeds the portable argument limit during {operation}")]
    CommandTooLong { operation: &'static str },
    #[error("local Docker worker disappeared during inspection")]
    ResourceAbsent,
    #[error("local Docker command timed out during {operation}")]
    CommandTimedOut { operation: &'static str },
    #[error("local Docker command failed during {operation}")]
    CommandFailed { operation: &'static str },
    #[error("local Docker returned a malformed response during {operation}")]
    InvalidResponse { operation: &'static str },
    #[error("local Docker returned an invalid or mismatched resource identity")]
    ResourceIdentityMismatch,
    #[error("local Docker worker {resource_id} could not be verified or deleted after creation")]
    CreationCleanupFailed { resource_id: String },
    #[error("created local Docker worker {resource_id} has unexpected expiry metadata and requires manual inspection")]
    PersistentLifetimeMetadataConflict { resource_id: String },
    #[error("persistent local Docker worker {resource_id} requires reconciliation after an uncertain create response")]
    PersistentCreationReconciliationRequired { resource_id: String },
    #[error("local Docker worker {resource_id} had an out-of-bounds lease and was deleted")]
    LeaseDeadlineRejected { resource_id: String },
    #[error("local Docker worker {resource_id} had an out-of-bounds lease but cleanup failed")]
    LeaseRejectionCleanupFailed { resource_id: String },
    #[error("local Docker worker has an invalid SSH endpoint")]
    InvalidSshEndpoint,
    #[error("local Docker worker has an invalid SSH host key")]
    InvalidHostKey,
    #[error("local Docker worker {resource_id} deletion could not be verified")]
    DeletionVerificationFailed { resource_id: String },
}

trait DockerTransport: Send + Sync {
    fn inspect(&self, reference: &str) -> DockerResult<Option<DockerContainer>>;
    fn inspect_with_timeout(&self, reference: &str, timeout: Duration) -> DockerResult<Option<DockerContainer>>;
    fn create(&self, request: &DockerCreateRequest) -> DockerResult<String>;
    fn read_host_key(&self, resource_id: &str) -> DockerResult<Option<String>>;
    fn delete(&self, resource_id: &str) -> DockerResult<bool>;
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DockerContainer {
    id: String,
    name: String,
    image: String,
    labels: BTreeMap<String, String>,
    environment: Vec<String>,
    restart_policy: String,
    running: bool,
    state: String,
    exit_code: i64,
    ssh_bindings: Vec<DockerPortBinding>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DockerPortBinding {
    host: String,
    port: u16,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct DockerCreateRequest {
    name: String,
    image: String,
    labels: BTreeMap<String, String>,
    ssh_public_key: String,
    terminate_after: Option<String>,
}

impl DockerCreateRequest {
    fn new(request: &InteractiveWorkerRequest, name: &str) -> DockerResult<Self> {
        let terminate_after = request
            .target
            .lifetime
            .time_limit_seconds()
            .map(termination_deadline)
            .transpose()?;
        let ssh_public_key = canonical_ed25519_key(&request.ssh_public_key).ok_or(LocalDockerError::InvalidTarget)?;
        let mut labels = request_labels(request)?;
        labels.insert(SSH_KEY_LABEL.to_string(), ssh_public_key.clone());
        if let Some(deadline) = &terminate_after {
            labels.insert(TERMINATE_LABEL.to_string(), deadline.clone());
        }
        Ok(Self {
            name: name.to_string(),
            image: request.target.image.clone(),
            labels,
            ssh_public_key,
            terminate_after,
        })
    }
}
fn termination_deadline(seconds: u32) -> DockerResult<String> {
    time::OffsetDateTime::now_utc()
        .checked_add(time::Duration::seconds(i64::from(seconds)))
        .ok_or(LocalDockerError::InvalidTarget)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| LocalDockerError::InvalidTarget)
}
fn validate_request(request: &InteractiveWorkerRequest, profile: &LocalDockerProfile) -> DockerResult<()> {
    (request.target.profile == profile.name && request.is_valid_for(CloudProvider::LocalDocker))
        .then_some(())
        .ok_or(LocalDockerError::InvalidTarget)
}
fn valid_local_docker_host(value: &str) -> bool {
    let safe = |path: &str| !path.is_empty() && !path.chars().any(char::is_control);
    value
        .strip_prefix("unix://")
        .is_some_and(|path| path.len() > 1 && path.starts_with('/') && safe(path))
        || value.strip_prefix("npipe:////./pipe/").is_some_and(safe)
}
fn validate_worker(worker: &InteractiveWorker) -> DockerResult<()> {
    (worker.is_valid_for(CloudProvider::LocalDocker) && valid_container_id(&worker.identity.resource_id))
        .then_some(())
        .ok_or(LocalDockerError::InvalidPersistedWorker)
}
fn worker_for_request(
    container: &DockerContainer,
    request: &InteractiveWorkerRequest,
) -> DockerResult<InteractiveWorker> {
    let lifetime = match request.target.lifetime {
        WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
        WorkerLifetime::TimeLimited { .. } => InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: required_environment(container, TERMINATE_ENV)?.to_string(),
        }),
    };
    let worker = InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: CloudProvider::LocalDocker,
            workflow_id: request.workflow_id,
            job_id: request.job_id,
            resource_id: container.id.clone(),
        },
        target: request.target.clone(),
        ssh_public_key: request.ssh_public_key.clone(),
        lifetime,
    };
    verify_container_for_worker(container, &worker)?;
    Ok(worker)
}
fn verify_container_for_worker(container: &DockerContainer, worker: &InteractiveWorker) -> DockerResult<()> {
    validate_worker(worker)?;
    let terminate_after = worker
        .lifetime
        .as_time_limited()
        .map(|lease| lease.terminate_after.as_str());
    let ssh_public_key =
        canonical_ed25519_key(&worker.ssh_public_key).ok_or(LocalDockerError::InvalidPersistedWorker)?;
    let expected_name = container_name(worker.identity.workflow_id, worker.identity.job_id);
    let exact_labels = [
        (MANAGED_LABEL, "true".to_string()),
        (PROVIDER_LABEL, "local_docker".to_string()),
        (PROTOCOL_LABEL, CLOUD_RUN_PROTOCOL_VERSION.to_string()),
        (WORKFLOW_LABEL, worker.identity.workflow_id.to_string()),
        (JOB_LABEL, worker.identity.job_id.to_string()),
        (SSH_KEY_LABEL, ssh_public_key.clone()),
    ];
    let valid = container.id == worker.identity.resource_id
        && container.name == expected_name
        && container.image == worker.target.image
        && container.restart_policy == "no"
        && exact_labels
            .iter()
            .all(|(key, value)| container.labels.get(*key) == Some(value))
        && required_environment(container, SSH_PUBLIC_KEY_ENV) == Ok(ssh_public_key.as_str())
        && lifetime_matches(container, terminate_after)
        && valid_target_label(&container.labels, worker);
    valid.then_some(()).ok_or(LocalDockerError::ResourceIdentityMismatch)
}
fn valid_target_label(labels: &BTreeMap<String, String>, worker: &InteractiveWorker) -> bool {
    labels
        .get(TARGET_LABEL)
        .and_then(|value| serde_json::from_str::<WorkerTarget>(value).ok())
        .is_some_and(|target| target == worker.target)
}
fn request_labels(request: &InteractiveWorkerRequest) -> DockerResult<BTreeMap<String, String>> {
    let target = serde_json::to_string(&request.target).map_err(|_| LocalDockerError::InvalidTarget)?;
    Ok(BTreeMap::from([
        (MANAGED_LABEL.to_string(), "true".to_string()),
        (PROVIDER_LABEL.to_string(), "local_docker".to_string()),
        (PROTOCOL_LABEL.to_string(), CLOUD_RUN_PROTOCOL_VERSION.to_string()),
        (WORKFLOW_LABEL.to_string(), request.workflow_id.to_string()),
        (JOB_LABEL.to_string(), request.job_id.to_string()),
        (TARGET_LABEL.to_string(), target),
    ]))
}
fn required_environment<'a>(container: &'a DockerContainer, key: &str) -> DockerResult<&'a str> {
    let prefix = format!("{key}=");
    let mut entries = container
        .environment
        .iter()
        .filter(|entry| environment_key_matches(entry, key));
    let value = entries
        .next()
        .and_then(|entry| entry.strip_prefix(&prefix))
        .ok_or(LocalDockerError::ResourceIdentityMismatch)?;
    if entries.next().is_some() {
        return Err(LocalDockerError::ResourceIdentityMismatch);
    }
    Ok(value)
}
fn environment_key_matches(entry: &str, key: &str) -> bool {
    entry
        .strip_prefix(key)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('='))
}
fn lifetime_matches(container: &DockerContainer, terminate_after: Option<&str>) -> bool {
    container.labels.get(TERMINATE_LABEL).map(String::as_str) == terminate_after
        && match terminate_after {
            Some(deadline) => required_environment(container, TERMINATE_ENV) == Ok(deadline),
            None => !container
                .environment
                .iter()
                .any(|entry| environment_key_matches(entry, TERMINATE_ENV)),
        }
}
fn created_container_matches(container: &DockerContainer, create: &DockerCreateRequest) -> bool {
    created_container_identity_matches(container, create)
        && lifetime_matches(container, create.terminate_after.as_deref())
}
fn persistent_reconciliation_error(
    container: &DockerContainer,
    create: &DockerCreateRequest,
    original: LocalDockerError,
) -> LocalDockerError {
    if !created_container_identity_matches(container, create) {
        return original;
    }
    let resource_id = container.id.clone();
    if lifetime_matches(container, None) {
        LocalDockerError::PersistentCreationReconciliationRequired { resource_id }
    } else {
        LocalDockerError::PersistentLifetimeMetadataConflict { resource_id }
    }
}
fn created_container_identity_matches(container: &DockerContainer, create: &DockerCreateRequest) -> bool {
    valid_container_id(&container.id)
        && container.name == create.name
        && container.image == create.image
        && container.restart_policy == "no"
        && create
            .labels
            .iter()
            .all(|(key, value)| container.labels.get(key) == Some(value))
        && required_environment(container, SSH_PUBLIC_KEY_ENV) == Ok(create.ssh_public_key.as_str())
}
fn canonical_ed25519_key(value: &str) -> Option<String> {
    if !valid_ssh_public_key(value) {
        return None;
    }
    let mut fields = value.split_ascii_whitespace();
    Some(format!("{} {}", fields.next()?, fields.next()?))
}
fn container_name(workflow_id: super::CloudWorkflowId, job_id: super::CloudJobId) -> String {
    format!("horizon-local-{workflow_id}-{job_id}")
}
fn valid_container_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
fn invalid_response(operation: &'static str) -> LocalDockerError {
    LocalDockerError::InvalidResponse { operation }
}
