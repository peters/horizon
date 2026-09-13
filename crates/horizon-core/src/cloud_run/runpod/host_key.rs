//! Explicit initial trust and retained-pin paths; neither grants task admission.

use super::{
    ApiPod, CloudProvider, HOST_KEY_BOOTSTRAP_ENV, InteractiveWorkerRequest, RunPodApiKey, RunPodError,
    RunPodHostKeySource, RunPodLifecycle, RunPodSshEndpoint, RunPodWorker, Transport,
    http::RunPodHttp,
    interactive::runpod_worker,
    network_attachment::{self, NetworkBinding},
    status_from_resource,
};
use crate::cloud_run::{
    ArtifactDigest,
    interactive_worker::{InteractiveWorker, InteractiveWorkerLifetime, InteractiveWorkerSshEndpoint},
};

pub(super) mod sample;
pub(super) const VERSION: u32 = 1;
#[cfg(test)]
mod tests;

/// A caller-selected trust mode, never inferred from a missing saved pin.
/// Provider inspection and this synchronous source must run off the render thread.
pub struct RunPodHostTrust {
    expected: InteractiveWorkerRequest,
    mode: Mode,
    network_binding: Option<NetworkBinding>,
}

enum Mode {
    InitialTaskFree(RunPodHttp),
    Retained {
        pod_id: String,
        lifetime: InteractiveWorkerLifetime,
        ssh: InteractiveWorkerSshEndpoint,
    },
}

impl RunPodHostTrust {
    /// Select first-pin bootstrap for this exact expected request.
    ///
    /// The caller asserts a controller-created Pod has run only the approved image
    /// entrypoint, with no arbitrary setup/task before first pin persistence. Do not
    /// call this constructor to recover lost trust for a previously used worker.
    /// The returned key must pass the existing owned snapshot/CAS before any task.
    /// Logs are provider-origin evidence, not boot freshness or complete history.
    /// # Errors
    /// Rejects an invalid expected provider/request before any I/O.
    pub fn initial_task_free(api_key: &RunPodApiKey, expected: &InteractiveWorkerRequest) -> Result<Self, RunPodError> {
        if !expected.is_valid_for(CloudProvider::RunPod) {
            return Err(RunPodError::InvalidTarget);
        }
        Ok(Self {
            expected: expected.clone(),
            mode: Mode::InitialTaskFree(RunPodHttp::new(api_key)),
            network_binding: None,
        })
    }

    /// Use an already-retained full SSH pin without any log/HTTP access here.
    /// The outer provider still freshly inspects ownership; the actual pinned SSH
    /// handshake proves possession. Endpoint migration and key rotation are refused.
    /// # Errors
    /// Rejects incomplete saved identity or trust; absence never selects bootstrap.
    pub fn retained(worker: &InteractiveWorker, ssh: &InteractiveWorkerSshEndpoint) -> Result<Self, RunPodError> {
        let retained = runpod_worker(worker)?;
        if !ssh.is_complete() {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        Ok(Self {
            expected: InteractiveWorkerRequest {
                workflow_id: worker.identity.workflow_id,
                job_id: worker.identity.job_id,
                target: worker.target.clone(),
                ssh_public_key: worker.ssh_public_key.clone(),
            },
            mode: Mode::Retained {
                pod_id: retained.pod_id,
                lifetime: retained.lifetime,
                ssh: ssh.clone(),
            },
            network_binding: None,
        })
    }

    pub(super) fn bind_network(&mut self, binding: &NetworkBinding) -> Result<(), RunPodError> {
        if self.expected != binding.request || self.network_binding.as_ref().is_some_and(|current| current != binding) {
            return Err(RunPodError::InvalidTarget);
        }
        self.network_binding = Some(binding.clone());
        Ok(())
    }

    fn observed(&self, pod: &ApiPod, worker: &RunPodWorker, endpoint: &RunPodSshEndpoint) -> bool {
        network_attachment::verify_attachment(pod, self.network_binding.as_ref().map(|binding| &binding.selection))
            .is_ok()
            && pod.env.get(HOST_KEY_BOOTSTRAP_ENV) == Some(&VERSION.to_string())
            && status_from_resource(pod, worker, Some(&self.expected.ssh_public_key)).is_ok_and(|status| {
                status.lifecycle == RunPodLifecycle::Running
                    && status.ssh_username.as_ref() == Some(&endpoint.username)
                    && status.ssh_host.as_ref() == Some(&endpoint.host)
                    && status.ssh_port == Some(endpoint.port)
            })
    }
}

impl RunPodHostKeySource for RunPodHostTrust {
    fn retained_endpoint(&self, worker: &InteractiveWorker) -> Option<InteractiveWorkerSshEndpoint> {
        let Mode::Retained { pod_id, lifetime, ssh } = &self.mode else {
            return None;
        };
        (worker.is_valid_for(CloudProvider::RunPod)
            && worker.identity.resource_id == *pod_id
            && worker.identity.workflow_id == self.expected.workflow_id
            && worker.identity.job_id == self.expected.job_id
            && worker.target == self.expected.target
            && worker.ssh_public_key == self.expected.ssh_public_key
            && worker.lifetime == *lifetime)
            .then(|| ssh.clone())
    }

    fn host_key(
        &self,
        worker: &RunPodWorker,
        endpoint: &RunPodSshEndpoint,
        expected_client_key: &str,
    ) -> Option<String> {
        if worker.validate().is_err()
            || worker.workflow_id != self.expected.workflow_id
            || worker.job_id != self.expected.job_id
            || worker.image != self.expected.target.image
            || !worker.lifetime.matches_policy(self.expected.target.lifetime)
            || expected_client_key != self.expected.ssh_public_key
        {
            return None;
        }
        match &self.mode {
            Mode::Retained { pod_id, lifetime, ssh } => (worker.pod_id == *pod_id
                && worker.lifetime == *lifetime
                && endpoint.host == ssh.host
                && endpoint.port == ssh.port
                && endpoint.username == ssh.username)
                .then(|| ssh.host_key.clone()),
            Mode::InitialTaskFree(http) => {
                let before = http.get(&worker.pod_id).ok()??;
                if !self.observed(&before, worker, endpoint) {
                    return None;
                }
                let bytes = http.host_key_sample(&worker.pod_id).ok()?;
                let key = sample::parse(&bytes, worker, &client_digest(expected_client_key)).ok()??;
                let after = http.get(&worker.pod_id).ok()??;
                (self.observed(&after, worker, endpoint)
                    && InteractiveWorkerSshEndpoint {
                        host: endpoint.host.clone(),
                        port: endpoint.port,
                        username: endpoint.username.clone(),
                        host_key: key.clone(),
                    }
                    .is_complete())
                .then_some(key)
            }
        }
    }
}

impl std::fmt::Debug for RunPodHostTrust {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RunPodHostTrust").finish_non_exhaustive()
    }
}

fn client_digest(key: &str) -> String {
    let normalized = key.split_ascii_whitespace().take(2).collect::<Vec<_>>().join(" ");
    ArtifactDigest::sha256(normalized.as_bytes()).as_str().to_string()
}
