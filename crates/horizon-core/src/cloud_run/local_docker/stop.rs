//! Exact-owned, data-retaining Stop; no creation, deletion or SSH path.

use super::{
    DockerContainer, DockerResult, InteractiveWorker, LocalDockerError, LocalDockerInteractiveWorkerProvider,
    verify_container_for_worker,
};
use crate::cloud_run::interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider};

impl InteractiveWorkerStopProvider for LocalDockerInteractiveWorkerProvider {
    fn stop_worker(&self, worker: &InteractiveWorker) -> DockerResult<InteractiveWorkerStop> {
        self.validate_persisted_worker(worker)?;
        let resource_id = &worker.identity.resource_id;
        let Some(before) = self.transport.inspect(resource_id)? else {
            return Ok(InteractiveWorkerStop::AlreadyAbsent);
        };
        if verify_stop_state(&before, worker)? {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        let stopping = self.transport.stop(resource_id);
        let after = self
            .transport
            .inspect(resource_id)?
            .ok_or(LocalDockerError::StopResourceLost)?;
        if verify_stop_state(&after, worker)? {
            return Ok(InteractiveWorkerStop::Stopped);
        }
        stopping?;
        Err(LocalDockerError::StopVerificationFailed)
    }
}

/// Returns true only for a known retained/inactive state, regardless of task exit code.
fn verify_stop_state(container: &DockerContainer, worker: &InteractiveWorker) -> DockerResult<bool> {
    verify_container_for_worker(container, worker)?;
    if container.auto_remove != Some(false) {
        return Err(LocalDockerError::StopRetentionUnverified);
    }
    match (container.state.as_str(), container.running) {
        ("created" | "exited", false) => Ok(true),
        ("running" | "paused" | "restarting", true) => Ok(false),
        _ => Err(LocalDockerError::StopStateUnverified),
    }
}
