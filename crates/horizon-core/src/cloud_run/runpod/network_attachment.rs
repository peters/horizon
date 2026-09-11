//! Request-bound attachment checks, not volume ownership or exclusivity proof.

use super::{
    ApiPod, CloudJobId, CloudProvider, CloudWorkflowId, CreatePodRequest, InteractiveWorkerRequest, RunPodClient,
    RunPodError, RunPodNetworkVolumeExpectation, RunPodProfile, WorkerLifetime, WorkerTarget, validate_target,
};
use crate::cloud_run::interactive_worker::InteractiveWorker;
use serde::Deserialize;

#[derive(Clone, Eq, PartialEq)]
pub(super) struct NetworkBinding {
    pub(super) request: InteractiveWorkerRequest,
    pub(super) selection: RunPodNetworkVolumeExpectation,
}

impl NetworkBinding {
    pub(super) fn new(
        request: &InteractiveWorkerRequest,
        selection: &RunPodNetworkVolumeExpectation,
        profile: &RunPodProfile,
    ) -> Result<Self, RunPodError> {
        validate_target(&request.target, profile)?;
        selection.validate()?;
        if !request.is_valid_for(CloudProvider::RunPod)
            || request.target.lifetime != WorkerLifetime::Persistent
            || profile
                .data_center_id
                .as_ref()
                .is_some_and(|id| id != &selection.data_center_id)
        {
            return Err(RunPodError::InvalidTarget);
        }
        Ok(Self {
            request: request.clone(),
            selection: selection.clone(),
        })
    }
}

impl RunPodClient {
    pub(super) fn bind_network(&mut self, binding: &NetworkBinding) -> Result<(), RunPodError> {
        if self.network_binding.as_ref().is_some_and(|current| current != binding) {
            return Err(RunPodError::InvalidTarget);
        }
        self.network_binding = Some(binding.clone());
        Ok(())
    }

    pub(super) fn check_network_request(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        key: Option<&str>,
    ) -> Result<(), RunPodError> {
        if let Some(binding) = &self.network_binding {
            let expected = &binding.request;
            if expected.workflow_id != workflow_id
                || expected.job_id != job_id
                || &expected.target != target
                || key != Some(expected.ssh_public_key.as_str())
            {
                return Err(RunPodError::InvalidTarget);
            }
        }
        Ok(())
    }

    pub(super) fn check_network_worker(&self, worker: &InteractiveWorker) -> Result<(), RunPodError> {
        if self.network_binding.is_some() && !worker.is_valid_for(CloudProvider::RunPod) {
            return Err(RunPodError::InvalidPersistedWorker);
        }
        self.check_network_request(
            worker.identity.workflow_id,
            worker.identity.job_id,
            &worker.target,
            Some(&worker.ssh_public_key),
        )
    }

    pub(super) fn require_unbound(&self) -> Result<(), RunPodError> {
        self.network_binding
            .is_none()
            .then_some(())
            .ok_or(RunPodError::InvalidTarget)
    }

    pub(super) fn verify_selected_volume(&self) -> Result<(), RunPodError> {
        if let Some(binding) = &self.network_binding {
            self.inspect_high_performance_volume(&binding.selection)?
                .ok_or(RunPodError::ResourceIdentityMismatch)?;
        }
        Ok(())
    }

    pub(super) fn apply_network_selection(&self, request: &mut CreatePodRequest) {
        if let Some(binding) = &self.network_binding {
            request.network_volume_id = Some(binding.selection.volume_id.clone());
            request.data_center_id = Some(binding.selection.data_center_id.clone());
            request.volume_in_gb = 0;
        }
    }

    pub(super) fn fresh_attachment(&self, pod: ApiPod) -> Result<ApiPod, RunPodError> {
        let current = if self.network_binding.is_some() {
            let current = self
                .transport
                .get(&pod.id)?
                .ok_or(RunPodError::ResourceIdentityMismatch)?;
            if current.id != pod.id {
                return Err(RunPodError::ResourceIdentityMismatch);
            }
            current
        } else {
            pod
        };
        self.verify_attachment(&current)?;
        Ok(current)
    }

    pub(super) fn verify_attachment(&self, pod: &ApiPod) -> Result<(), RunPodError> {
        verify_attachment(pod, self.network_binding.as_ref().map(|binding| &binding.selection))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkMounts {
    network: Vec<NetworkMount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct NetworkMount {
    volume_id: String,
    path: String,
}

pub(super) fn verify_attachment(
    pod: &ApiPod,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<(), RunPodError> {
    let mounts = pod.stop.mounts();
    let Some(selection) = selection else {
        // Unknown optional legacy metadata remains compatible; a network entry
        // (even malformed or empty) must never select ordinary adoption.
        return mounts
            .get("network")
            .is_none()
            .then_some(())
            .ok_or(RunPodError::ResourceIdentityMismatch);
    };
    let mounts: NetworkMounts =
        serde_json::from_value(mounts.clone()).map_err(|_| RunPodError::ResourceIdentityMismatch)?;
    if !pod.stop.is_secure_data_center(&selection.data_center_id)
        || mounts.network.len() != 1
        || mounts.network[0].volume_id != selection.volume_id
        || mounts.network[0].path != "/workspace"
    {
        return Err(RunPodError::ResourceIdentityMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
