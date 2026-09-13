//! Single-dispatch compute resume of an owned persistent Pod, never task replay.
use super::{
    ApiPod, InteractiveWorkerLifetime, RunPodError, RunPodInteractiveWorkerProvider, RunPodLifecycle, RunPodWorker,
    RunPodWorkerStatus, interactive::runpod_worker, status_from_resource, stop::RetainedMount, validate_target,
};
use crate::cloud_run::{
    interactive_worker::{InteractiveWorker, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus},
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
};
use std::time::Duration;

// At most three observations, each with the existing 30-second HTTP bound (plus
// one selected-volume read). Including admission and POST, at most 275 seconds
// of provider requests/backoff; no mutation retry or unbounded polling.
const OBSERVATION_BACKOFF_MS: [u64; 3] = [0, 1_000, 4_000];

struct Snapshot {
    status: RunPodWorkerStatus,
    mount: RetainedMount,
}

impl RunPodInteractiveWorkerProvider {
    fn start_status(
        &self,
        status: RunPodWorkerStatus,
        worker: &InteractiveWorker,
        pin: &InteractiveWorkerSshEndpoint,
    ) -> Result<InteractiveWorkerStatus, RunPodError> {
        let status = self.adapt_status(status, &worker.target, &worker.ssh_public_key);
        if status.ssh.as_ref().is_some_and(|ssh| ssh != pin) {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        Ok(status)
    }

    fn start_snapshot(
        &self,
        pod: &ApiPod,
        worker: &RunPodWorker,
        expected: &InteractiveWorker,
        pin: &InteractiveWorkerSshEndpoint,
    ) -> Result<Snapshot, RunPodError> {
        let status = status_from_resource(pod, worker, Some(&expected.ssh_public_key))?;
        let mount = self
            .client
            .retained_mount(pod, &self.profile)
            .map_err(|_| RunPodError::StartUnverified)?;
        let metadata = &pod.stop;
        let valid = match status.lifecycle {
            RunPodLifecycle::Exited => {
                metadata.runtime.as_ref().is_some_and(serde_json::Value::is_null)
                    && metadata.locked == false
                    && metadata
                        .actions
                        .as_array()
                        .is_some_and(|actions| actions.iter().any(|v| v == "start"))
            }
            RunPodLifecycle::Running => metadata.runtime.as_ref().is_some_and(serde_json::Value::is_object),
            RunPodLifecycle::Provisioning => {
                pod.status.as_deref() == Some("STARTING")
                    && metadata.runtime.as_ref().is_some_and(serde_json::Value::is_null)
            }
            _ => false,
        };
        if !valid {
            return Err(RunPodError::StartUnverified);
        }
        if let Some(ssh) = pod.ssh.as_ref().and_then(|ssh| ssh.direct.as_ref())
            && (ssh.host != pin.host || ssh.port != pin.port || ssh.username != pin.username)
        {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        Ok(Snapshot { status, mount })
    }
}

impl InteractiveWorkerStartProvider for RunPodInteractiveWorkerProvider {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        self.client.check_network_worker(worker)?;
        let retained = runpod_worker(worker)?;
        validate_target(&worker.target, &self.profile)?;
        if worker.lifetime != InteractiveWorkerLifetime::Persistent {
            return Err(RunPodError::StartIdentityRequired);
        }
        let pin = self
            .host_keys
            .retained_endpoint(worker)
            .filter(InteractiveWorkerSshEndpoint::is_complete)
            .ok_or(RunPodError::StartIdentityRequired)?;
        if self.client.network_binding.is_none() && self.profile.volume_gib == 0 {
            return Err(RunPodError::StartUnverified);
        }
        let Some(pod) = self.client.transport.get(&retained.pod_id)? else {
            return Ok(InteractiveWorkerStart::AlreadyAbsent);
        };
        let before = self.start_snapshot(&pod, &retained, worker, &pin)?;
        if before.status.lifecycle == RunPodLifecycle::Running {
            return self
                .start_status(before.status, worker, &pin)
                .map(InteractiveWorkerStart::AlreadyRunning);
        }
        // A STARTING Pod is only observed. An ambiguous POST response is never
        // authority to submit again; only independently observed state can succeed.
        if before.status.lifecycle == RunPodLifecycle::Exited {
            let _response = self.client.transport.start(&retained.pod_id);
        }
        for delay_ms in OBSERVATION_BACKOFF_MS {
            if !cfg!(test) {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            let Some(pod) = self.client.transport.get(&retained.pod_id)? else {
                return Err(RunPodError::StartUnverified);
            };
            let current = self.start_snapshot(&pod, &retained, worker, &pin)?;
            if current.mount != before.mount {
                return Err(RunPodError::StartUnverified);
            }
            if current.status.lifecycle == RunPodLifecycle::Running {
                return self
                    .start_status(current.status, worker, &pin)
                    .map(InteractiveWorkerStart::Started);
            }
        }
        Err(RunPodError::StartUnverified)
    }
}

#[cfg(test)]
mod tests;
