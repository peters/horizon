//! Durable creation authority is separate from resource inspection and client lifetime.

use super::{
    DockerResult, InteractiveWorkerEnsure, InteractiveWorkerProvider, InteractiveWorkerRequest, LocalDockerError,
    LocalDockerInteractiveWorkerProvider,
};

impl LocalDockerInteractiveWorkerProvider {
    pub(super) fn claim_creation(&self, request: &InteractiveWorkerRequest, name: &str) -> DockerResult<bool> {
        self.creation_store
            .claim_worker_creation(request.workflow_id, request.job_id, &request.target, name)
            .map_err(|_| LocalDockerError::CreationFenceFailed)
    }

    pub(super) fn create_once(
        &self,
        request: &InteractiveWorkerRequest,
        name: &str,
    ) -> DockerResult<InteractiveWorkerEnsure> {
        let granted = self.claim_creation(request, name)?;
        if !granted {
            // Another controller may have completed creation since the first lookup.
            // This retry is read-only even if the resource is stopped, missing or expired.
            return self
                .reconcile_worker(request)?
                .map(InteractiveWorkerEnsure::Reused)
                .ok_or(LocalDockerError::CreationReconciliationRequired);
        }
        // Keep the consumed grant after every outcome, including an uncertain response.
        self.create_worker(request, name)
    }
}
