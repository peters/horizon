//! Configured Azure saved-Stop admission: the exact named profile, its immutable CPU
//! binding, the retained persistent worker and the complete public pin are bound before
//! the CLI credential or client exists. Observation only: no Stop, Start, creation,
//! deletion, private-key repair or host-key attestation happens here.

use super::{
    ConfiguredStopConfirmation, ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopConfirmation,
    RemoteWorkspaceStopError, current_millis,
};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, RemoteCpuProfileBinding, StoredRemoteAllocation, WorkerLifetime,
        azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile},
        interactive_worker::InteractiveWorkerRequest,
        interactive_worker_stop::InteractiveWorkerStopObserver,
    },
    remote_workspace::RemoteEnvironmentSummary,
};

/// One admitted Azure allocation: exact summary, valid public request, persistent
/// target within the named profile, immutable binding to that profile, no `RunPod`
/// storage expectation, no management intent, and a retained worker with a complete pin.
pub(super) struct RetainedAzure {
    pub(super) allocation: StoredRemoteAllocation,
    request: InteractiveWorkerRequest,
    binding: RemoteCpuProfileBinding,
}

impl RetainedAzure {
    pub(super) fn load(
        store: &CloudWorkflowStore,
        profile: &AzureProfile,
        expected: &RemoteEnvironmentSummary,
    ) -> Result<Self, BindingError> {
        let allocation = store
            .load_remote_allocation(&expected.owning_session_id, &expected.workspace_local_id)
            .map_err(RemoteWorkspaceStopError::from)?
            .ok_or(RemoteWorkspaceStopError::MissingAllocation)?;
        if allocation.workspace().environment_summary() != *expected {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        let request = allocation.worker_request().map_err(RemoteWorkspaceStopError::from)?;
        if !request.is_valid_for(CloudProvider::Azure) {
            return Err(BindingError::InvalidBinding);
        }
        if request.target.lifetime != WorkerLifetime::Persistent {
            return Err(RemoteWorkspaceStopError::UnsupportedLifetime.into());
        }
        AzureDeploymentPlan::validate_target(profile, &request.target).map_err(|_| BindingError::InvalidBinding)?;
        let runtime = allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(BindingError::InvalidBinding)?;
        if runtime.cleanup.is_some() {
            return Err(RemoteWorkspaceStopError::ManagementConflict.into());
        }
        // The worker was created under one immutable profile binding; the named profile
        // must still be that exact profile (subscription and every approved field).
        // Missing provenance is never backfilled from the current configuration.
        let binding = store
            .load_remote_cpu_profile_binding(&allocation)
            .map_err(RemoteWorkspaceStopError::from)?
            .ok_or(BindingError::InvalidBinding)?;
        if !binding
            .matches_profile(profile)
            .map_err(|_| BindingError::InvalidBinding)?
        {
            return Err(BindingError::InvalidBinding);
        }
        // An Azure worker carries no RunPod volume expectation; a saved selection is a
        // foreign binding and is refused rather than passed to the observer.
        if store
            .load_remote_network_volume_selection(&allocation)
            .map_err(RemoteWorkspaceStopError::from)?
            .is_some()
        {
            return Err(BindingError::InvalidBinding);
        }
        let admitted = Self {
            allocation,
            request,
            binding,
        };
        admitted.trust()?;
        Ok(admitted)
    }

    /// The retained worker is the reserved request's worker and its saved pin is complete.
    /// The pin is saved and shape-checked, never attested here.
    fn trust(&self) -> Result<(), BindingError> {
        let runtime = self
            .allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(BindingError::InvalidBinding)?;
        let worker = runtime.worker.as_ref().ok_or(RemoteWorkspaceStopError::MissingWorker)?;
        let ssh = runtime.ssh.as_ref().ok_or(RemoteWorkspaceStopError::MissingTrust)?;
        if worker.identity.workflow_id != self.request.workflow_id
            || worker.identity.job_id != self.request.job_id
            || worker.target != self.request.target
            || worker.ssh_public_key != self.request.ssh_public_key
            || !worker.is_valid_for(CloudProvider::Azure)
        {
            return Err(BindingError::InvalidBinding);
        }
        if !ssh.is_complete() {
            return Err(RemoteWorkspaceStopError::MissingTrust.into());
        }
        Ok(())
    }

    /// The production observer: the CLI credential pinned to the profile's subscription
    /// and the Azure client over it. Both are lazy; no token is requested or persisted
    /// here, and a missing CLI login surfaces as an unverified observation. Callers
    /// reach this only through [`azure_with`], after admission.
    pub(super) fn client(store: &CloudWorkflowStore, profile: &AzureProfile) -> Result<AzureClient, BindingError> {
        let credential =
            AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| BindingError::InvalidBinding)?;
        AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| BindingError::InvalidBinding)
    }

    /// The exact allocation, its binding and the absence of a storage selection must
    /// all still hold: any drift around the observation is a changed state, not a result.
    pub(super) fn check_current(
        &self,
        store: &CloudWorkflowStore,
        expected: &StoredRemoteAllocation,
    ) -> Result<(), BindingError> {
        let current = store
            .load_remote_allocation(
                expected.workspace().session_id(),
                &expected.workspace().state().spec.workspace_local_id,
            )
            .map_err(RemoteWorkspaceStopError::from)?;
        if current.as_ref() != Some(expected)
            || store
                .load_remote_cpu_profile_binding(expected)
                .map_err(RemoteWorkspaceStopError::from)?
                .as_ref()
                != Some(&self.binding)
            || store
                .load_remote_network_volume_selection(expected)
                .map_err(RemoteWorkspaceStopError::from)?
                .is_some()
        {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        Ok(())
    }
}

/// Admission, then the observer, then the shared coordinator, with the saved state
/// rechecked before the observer exists and after the observation returned, even on
/// error. `client` and `confirm` are injectable so tests run the real ordering without
/// the Azure CLI or ARM.
pub(super) fn azure_with<P: InteractiveWorkerStopObserver>(
    store: &CloudWorkflowStore,
    profile: &AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    confirm: impl FnOnce(&P, &StoredRemoteAllocation) -> Result<RemoteWorkspaceStopConfirmation, RemoteWorkspaceStopError>,
) -> Result<ConfiguredStopConfirmation, BindingError> {
    let admitted = RetainedAzure::load(store, profile, expected)?;
    let requested = admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.phase.stop_requested_at_millis())
        .ok_or(RemoteWorkspaceStopError::MissingStopIntent)?;
    if requested > current_millis()? {
        return Err(RemoteWorkspaceStopError::InvalidTimestamp.into());
    }
    // The client (and its lazy credential) is built first, then the saved state is
    // rechecked, so drift during that step is a changed state even when the client
    // could not be built.
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let result = confirm(&provider?, &admitted.allocation);
    let current = result
        .as_ref()
        .map_or(&admitted.allocation, |result| &result.allocation);
    admitted.check_current(store, current)?;
    let result = result?;
    Ok(ConfiguredStopConfirmation {
        saved: result.allocation.workspace().environment_summary(),
        observation: result.observation,
    })
}
