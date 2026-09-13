use super::super::{
    CloudProvider, WorkerTarget,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
        InteractiveWorkerLifecycle, InteractiveWorkerProvider, InteractiveWorkerRequest, InteractiveWorkerSshEndpoint,
        InteractiveWorkerStatus, valid_ssh_coordinates,
    },
    interactive_worker_stop::{
        InteractiveWorkerStop, InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation,
        InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
    },
};
use super::{
    RunPodCleanup, RunPodClient, RunPodEnsure, RunPodError, RunPodHostTrust, RunPodLifecycle,
    RunPodNetworkVolumeExpectation, RunPodProfile, RunPodSshEndpoint, RunPodWorker, RunPodWorkerStatus,
    network_attachment::NetworkBinding, resource_name,
};

/// Trusted source for the runtime SSH host key of one exact worker.
///
/// Returning `None` keeps a running worker in `Provisioning`. Implementations
/// must authenticate that the returned key belongs to the supplied exact
/// worker identity and endpoint. An unauthenticated network key scan is not an
/// attestation source.
pub trait RunPodHostKeySource: Send + Sync {
    /// Return only an already-retained pin for this exact worker, without I/O or
    /// bootstrap. The default denies compute Start to initial-trust sources.
    #[must_use]
    fn retained_endpoint(&self, _worker: &InteractiveWorker) -> Option<InteractiveWorkerSshEndpoint> {
        None
    }

    #[must_use]
    fn host_key(
        &self,
        worker: &RunPodWorker,
        endpoint: &RunPodSshEndpoint,
        expected_client_key: &str,
    ) -> Option<String>;
}

impl<F> RunPodHostKeySource for F
where
    F: Fn(&RunPodWorker, &RunPodSshEndpoint, &str) -> Option<String> + Send + Sync,
{
    fn host_key(
        &self,
        worker: &RunPodWorker,
        endpoint: &RunPodSshEndpoint,
        expected_client_key: &str,
    ) -> Option<String> {
        self(worker, endpoint, expected_client_key)
    }
}

/// Adapts the exact GPU-worker lifecycle to the common interactive contract.
pub struct RunPodInteractiveWorkerProvider {
    pub(super) client: RunPodClient,
    pub(super) profile: RunPodProfile,
    pub(super) host_keys: Box<dyn RunPodHostKeySource>,
}

impl RunPodInteractiveWorkerProvider {
    /// Bind one caller-authorized selection to the complete persistent request.
    /// The caller must retain this selection before creation and reuse it on recovery.
    /// This does not prove volume ownership, exclusivity, contents or durability,
    /// and grants no volume mutation. Explicit Stop verifies retained attachment separately.
    /// # Errors
    /// Rejects invalid or conflicting requests, profiles and prior bindings before I/O.
    pub fn new_with_network_volume(
        mut client: RunPodClient,
        profile: RunPodProfile,
        mut trust: RunPodHostTrust,
        expected: &InteractiveWorkerRequest,
        selection: &RunPodNetworkVolumeExpectation,
    ) -> Result<Self, RunPodError> {
        let binding = NetworkBinding::new(expected, selection, &profile)?;
        client.bind_network(&binding)?;
        trust.bind_network(&binding)?;
        Ok(Self::new(client, profile, trust))
    }

    #[must_use]
    pub fn new(client: RunPodClient, profile: RunPodProfile, host_keys: impl RunPodHostKeySource + 'static) -> Self {
        Self {
            client,
            profile,
            host_keys: Box::new(host_keys),
        }
    }

    pub(super) fn adapt_status(
        &self,
        status: RunPodWorkerStatus,
        target: &WorkerTarget,
        ssh_public_key: &str,
    ) -> InteractiveWorkerStatus {
        let (lifecycle, ssh) = self.adapt_connection(&status, ssh_public_key);
        InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::RunPod,
                    workflow_id: status.worker.workflow_id,
                    job_id: status.worker.job_id,
                    resource_id: status.worker.pod_id,
                },
                target: target.clone(),
                ssh_public_key: ssh_public_key.to_string(),
                lifetime: status.worker.lifetime,
            },
            lifecycle,
            ssh,
        }
    }

    fn adapt_connection(
        &self,
        status: &RunPodWorkerStatus,
        expected_client_key: &str,
    ) -> (InteractiveWorkerLifecycle, Option<InteractiveWorkerSshEndpoint>) {
        match status.lifecycle {
            RunPodLifecycle::Provisioning => (InteractiveWorkerLifecycle::Provisioning, None),
            RunPodLifecycle::Exited | RunPodLifecycle::Terminated => (InteractiveWorkerLifecycle::Stopped, None),
            RunPodLifecycle::Failed => (InteractiveWorkerLifecycle::Failed, None),
            RunPodLifecycle::Unknown => (InteractiveWorkerLifecycle::Unknown, None),
            RunPodLifecycle::Running => self.adapt_running_connection(status, expected_client_key),
        }
    }

    fn adapt_running_connection(
        &self,
        status: &RunPodWorkerStatus,
        expected_client_key: &str,
    ) -> (InteractiveWorkerLifecycle, Option<InteractiveWorkerSshEndpoint>) {
        let Some((username, host, port)) = status
            .ssh_username
            .as_ref()
            .zip(status.ssh_host.as_ref())
            .zip(status.ssh_port)
            .map(|((username, host), port)| (username, host, port))
        else {
            return (InteractiveWorkerLifecycle::Provisioning, None);
        };
        let endpoint = RunPodSshEndpoint {
            username: username.clone(),
            host: host.clone(),
            port,
        };
        if !valid_ssh_coordinates(&endpoint.host, endpoint.port, &endpoint.username) {
            return (InteractiveWorkerLifecycle::Failed, None);
        }
        let Some(host_key) = self.host_keys.host_key(&status.worker, &endpoint, expected_client_key) else {
            return (InteractiveWorkerLifecycle::Provisioning, None);
        };
        let endpoint = InteractiveWorkerSshEndpoint {
            host: endpoint.host.clone(),
            port: endpoint.port,
            username: endpoint.username.clone(),
            host_key,
        };
        if endpoint.is_complete() {
            (InteractiveWorkerLifecycle::Ready, Some(endpoint))
        } else {
            (InteractiveWorkerLifecycle::Failed, None)
        }
    }
}

impl InteractiveWorkerProvider for RunPodInteractiveWorkerProvider {
    type Error = RunPodError;

    fn provider(&self) -> CloudProvider {
        CloudProvider::RunPod
    }

    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        if !request.is_valid_for(self.provider()) {
            super::diagnostics::ensure_failed(&RunPodError::InvalidTarget);
            return Err(RunPodError::InvalidTarget);
        }
        let ensured = self
            .client
            .ensure_interactive_worker(
                request.workflow_id,
                request.job_id,
                &request.target,
                &self.profile,
                &request.ssh_public_key,
            )
            .inspect_err(super::diagnostics::ensure_failed)?;
        Ok(match ensured {
            RunPodEnsure::Created(status) => {
                InteractiveWorkerEnsure::Created(self.adapt_status(status, &request.target, &request.ssh_public_key))
            }
            RunPodEnsure::Reused(status) => {
                InteractiveWorkerEnsure::Reused(self.adapt_status(status, &request.target, &request.ssh_public_key))
            }
        })
    }

    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.client.check_network_worker(worker)?;
        let target = worker.target.clone();
        let ssh_public_key = worker.ssh_public_key.clone();
        let worker = runpod_worker(worker)?;
        self.client
            .inspect_interactive_worker(&worker, &ssh_public_key)
            .map(|status| status.map(|status| self.adapt_status(status, &target, &ssh_public_key)))
    }

    fn reconcile_worker(
        &self,
        request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.client
            .reconcile_interactive_worker(request, &self.profile)
            .map(|status| status.map(|status| self.adapt_status(status, &request.target, &request.ssh_public_key)))
    }

    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.client.check_network_worker(worker)?;
        let ssh_public_key = worker.ssh_public_key.clone();
        let worker = runpod_worker(worker)?;
        Ok(match self.client.delete_interactive_worker(&worker, &ssh_public_key)? {
            RunPodCleanup::Deleted => InteractiveWorkerCleanup::Deleted,
            RunPodCleanup::AlreadyAbsent => InteractiveWorkerCleanup::AlreadyAbsent,
        })
    }
}

impl InteractiveWorkerStopProvider for RunPodInteractiveWorkerProvider {
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error> {
        self.client.check_network_worker(worker)?;
        let retained = runpod_worker(worker)?;
        super::validate_target(&worker.target, &self.profile)?;
        self.client
            .stop_interactive_worker(&retained, &worker.ssh_public_key, &self.profile)
    }
}

impl InteractiveWorkerStopObserver for RunPodInteractiveWorkerProvider {
    fn observe_worker_stop(
        &self,
        expected: InteractiveWorkerStopExpectation<'_>,
    ) -> Result<InteractiveWorkerStopObservation, Self::Error> {
        self.client.check_network_worker(expected.worker)?;
        let retained = runpod_worker(expected.worker)?;
        super::validate_target(&expected.worker.target, &self.profile)?;
        if !expected.ssh.is_complete() {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        if self.client.network_binding.as_ref().map(|binding| &binding.selection) != expected.network_volume {
            return Err(RunPodError::InvalidTarget);
        }
        self.client
            .observe_interactive_worker_stop(&retained, &expected.worker.ssh_public_key, &self.profile)
    }
}

pub(super) fn runpod_worker(worker: &InteractiveWorker) -> Result<RunPodWorker, RunPodError> {
    if !worker.is_valid_for(CloudProvider::RunPod) {
        return Err(RunPodError::InvalidPersistedWorker);
    }
    let runpod_worker = RunPodWorker {
        workflow_id: worker.identity.workflow_id,
        job_id: worker.identity.job_id,
        pod_id: worker.identity.resource_id.clone(),
        name: resource_name(worker.identity.workflow_id, worker.identity.job_id),
        image: worker.target.image.clone(),
        lifetime: worker.lifetime.clone(),
        hourly_cost_micros: None,
    };
    runpod_worker.validate()?;
    Ok(runpod_worker)
}
