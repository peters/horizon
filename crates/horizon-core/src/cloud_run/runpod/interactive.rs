use super::super::{
    CloudProvider,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
        InteractiveWorkerLease, InteractiveWorkerLifecycle, InteractiveWorkerProvider, InteractiveWorkerRequest,
        InteractiveWorkerSshEndpoint, InteractiveWorkerStatus, valid_ssh_coordinates,
    },
};
use super::{
    RunPodCleanup, RunPodClient, RunPodEnsure, RunPodError, RunPodLifecycle, RunPodProfile, RunPodSshEndpoint,
    RunPodWorker, RunPodWorkerStatus, resource_name,
};

/// Trusted source for the runtime SSH host key of one exact worker.
///
/// Returning `None` keeps a running worker in `Provisioning`. Implementations
/// must authenticate that the returned key belongs to the supplied exact
/// worker identity and endpoint. An unauthenticated network key scan is not an
/// attestation source.
pub trait RunPodHostKeySource: Send + Sync {
    #[must_use]
    fn host_key(&self, worker: &RunPodWorker, endpoint: &RunPodSshEndpoint) -> Option<String>;
}

impl<F> RunPodHostKeySource for F
where
    F: Fn(&RunPodWorker, &RunPodSshEndpoint) -> Option<String> + Send + Sync,
{
    fn host_key(&self, worker: &RunPodWorker, endpoint: &RunPodSshEndpoint) -> Option<String> {
        self(worker, endpoint)
    }
}

/// Adapts the exact GPU-worker lifecycle to the common interactive contract.
pub struct RunPodInteractiveWorkerProvider {
    client: RunPodClient,
    profile: RunPodProfile,
    host_keys: Box<dyn RunPodHostKeySource>,
}

impl RunPodInteractiveWorkerProvider {
    #[must_use]
    pub fn new(client: RunPodClient, profile: RunPodProfile, host_keys: impl RunPodHostKeySource + 'static) -> Self {
        Self {
            client,
            profile,
            host_keys: Box::new(host_keys),
        }
    }

    fn adapt_status(&self, status: RunPodWorkerStatus) -> Result<InteractiveWorkerStatus, RunPodError> {
        let ssh_public_key = status
            .worker
            .ssh_public_key
            .clone()
            .ok_or(RunPodError::InvalidPersistedWorker)?;
        let (lifecycle, ssh) = self.adapt_connection(&status);
        Ok(InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::RunPod,
                    workflow_id: status.worker.workflow_id,
                    job_id: status.worker.job_id,
                    resource_id: status.worker.pod_id,
                },
                image: status.worker.image,
                ssh_public_key,
                lease: InteractiveWorkerLease {
                    terminate_after: status.worker.terminate_after,
                },
            },
            lifecycle,
            ssh,
        })
    }

    fn adapt_connection(
        &self,
        status: &RunPodWorkerStatus,
    ) -> (InteractiveWorkerLifecycle, Option<InteractiveWorkerSshEndpoint>) {
        match status.lifecycle {
            RunPodLifecycle::Provisioning => (InteractiveWorkerLifecycle::Provisioning, None),
            RunPodLifecycle::Exited | RunPodLifecycle::Terminated => (InteractiveWorkerLifecycle::Stopped, None),
            RunPodLifecycle::Failed => (InteractiveWorkerLifecycle::Failed, None),
            RunPodLifecycle::Unknown => (InteractiveWorkerLifecycle::Unknown, None),
            RunPodLifecycle::Running => self.adapt_running_connection(status),
        }
    }

    fn adapt_running_connection(
        &self,
        status: &RunPodWorkerStatus,
    ) -> (InteractiveWorkerLifecycle, Option<InteractiveWorkerSshEndpoint>) {
        let Some(endpoint) = status.ssh.as_ref() else {
            return (InteractiveWorkerLifecycle::Provisioning, None);
        };
        if !valid_ssh_coordinates(&endpoint.host, endpoint.port, &endpoint.username) {
            return (InteractiveWorkerLifecycle::Failed, None);
        }
        let Some(host_key) = self.host_keys.host_key(&status.worker, endpoint) else {
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
            return Err(RunPodError::InvalidTarget);
        }
        let ensured = self.client.ensure_interactive_worker(
            request.workflow_id,
            request.job_id,
            &request.target,
            &self.profile,
            &request.ssh_public_key,
        )?;
        Ok(match ensured {
            RunPodEnsure::Created(status) => InteractiveWorkerEnsure::Created(self.adapt_status(status)?),
            RunPodEnsure::Reused(status) => InteractiveWorkerEnsure::Reused(self.adapt_status(status)?),
        })
    }

    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        let worker = runpod_worker(worker)?;
        self.client
            .inspect_worker(&worker)?
            .map(|status| self.adapt_status(status))
            .transpose()
    }

    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        let worker = runpod_worker(worker)?;
        Ok(match self.client.delete_worker(&worker)? {
            RunPodCleanup::Deleted => InteractiveWorkerCleanup::Deleted,
            RunPodCleanup::AlreadyAbsent => InteractiveWorkerCleanup::AlreadyAbsent,
        })
    }
}

fn runpod_worker(worker: &InteractiveWorker) -> Result<RunPodWorker, RunPodError> {
    if !worker.is_valid_for(CloudProvider::RunPod) {
        return Err(RunPodError::InvalidPersistedWorker);
    }
    let runpod_worker = RunPodWorker {
        workflow_id: worker.identity.workflow_id,
        job_id: worker.identity.job_id,
        pod_id: worker.identity.resource_id.clone(),
        name: resource_name(worker.identity.workflow_id, worker.identity.job_id),
        image: worker.image.clone(),
        terminate_after: worker.lease.terminate_after.clone(),
        ssh_public_key: Some(worker.ssh_public_key.clone()),
        hourly_cost_micros: None,
    };
    runpod_worker.validate()?;
    Ok(runpod_worker)
}
