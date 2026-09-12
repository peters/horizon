//! Configured first Stop and shared retained binding admission; no creation or SSH authority.

use super::RemoteWorkspaceStopError;
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

#[cfg(target_os = "linux")]
use {
    super::{ConfiguredStopConfirmationError as BindingError, stop_allocation},
    crate::cloud_run::{
        CloudProvider, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::InteractiveWorkerRequest,
        runpod::{
            RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider,
            RunPodNetworkVolumeExpectation, RunPodProfile, validate_target,
        },
    },
};

/// Send one explicitly confirmed Stop for an exact retained persistent `RunPod` worker.
/// Linux only; run off the render thread. Requires its named profile, complete saved
/// public pin and saved HPS selection before lazy credential lookup or provider work.
/// The pin is saved/shape-checked, not freshly attested; no private SSH key is needed.
/// Durable intent precedes the provider call. Once intent exists, this entry point
/// refuses another Stop: use the separate saved-Stop confirmation API after uncertainty.
/// Does not create, restart, delete, discover storage, poll or repair identity.
/// Verified Stop is point-in-time retained attachment, not a backup, task checkpoint,
/// process-memory preservation, filesystem durability or proof billing has ceased.
/// # Errors
/// Rejects unsupported, stale, missing or malformed selections, trust/storage/profile
/// drift, competing management, existing intent, missing credentials and unverified Stop.
/// Failures after dispatch may retain intent and identity; refresh before checking it.
pub fn stop_configured_runpod_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentSummary, ConfiguredRunPodStopError> {
    #[cfg(target_os = "linux")]
    {
        if expected.provider != CloudProvider::RunPod {
            return Err(ConfiguredRunPodStopError::UnsupportedProvider);
        }
        stop_with(
            store,
            config.runpod_profile(&expected.profile)?,
            expected,
            || RunPodApiKey::from_env().map_err(|_| ConfiguredRunPodStopError::CredentialUnavailable),
            |provider, allocation| stop_allocation(store, provider, allocation),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, expected);
        Err(ConfiguredRunPodStopError::UnsupportedProvider)
    }
}

#[cfg(target_os = "linux")]
pub(super) struct RetainedRunPod {
    pub(super) allocation: StoredRemoteAllocation,
    pub(super) selection: Option<RunPodNetworkVolumeExpectation>,
    request: InteractiveWorkerRequest,
}

#[cfg(target_os = "linux")]
impl RetainedRunPod {
    pub(super) fn load(
        store: &CloudWorkflowStore,
        profile: &RunPodProfile,
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
        if !request.is_valid_for(CloudProvider::RunPod) {
            return Err(BindingError::InvalidBinding);
        }
        validate_target(&request.target, profile).map_err(|_| BindingError::InvalidBinding)?;
        let runtime = allocation
            .workspace()
            .state()
            .runtime
            .as_ref()
            .ok_or(BindingError::InvalidBinding)?;
        if runtime.cleanup.is_some() {
            return Err(RemoteWorkspaceStopError::ManagementConflict.into());
        }
        if request.target.lifetime != WorkerLifetime::Persistent {
            return Err(RemoteWorkspaceStopError::UnsupportedLifetime.into());
        }
        let selection = store
            .load_remote_network_volume_selection(&allocation)
            .map_err(RemoteWorkspaceStopError::from)?;
        if let Some(selection) = &selection {
            selection.validate().map_err(|_| BindingError::InvalidBinding)?;
            if profile
                .data_center_id
                .as_ref()
                .is_some_and(|id| id != &selection.data_center_id)
            {
                return Err(BindingError::InvalidBinding);
            }
        } else if profile.volume_gib == 0 {
            return Err(BindingError::InvalidBinding);
        }
        let admitted = Self {
            allocation,
            selection,
            request,
        };
        admitted.trust()?;
        Ok(admitted)
    }

    fn trust(&self) -> Result<RunPodHostTrust, BindingError> {
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
        {
            return Err(BindingError::InvalidBinding);
        }
        RunPodHostTrust::retained(worker, ssh).map_err(|_| BindingError::InvalidBinding)
    }

    pub(super) fn provider(
        &self,
        store: &CloudWorkflowStore,
        profile: &RunPodProfile,
        key: &RunPodApiKey,
    ) -> Result<RunPodInteractiveWorkerProvider, BindingError> {
        let client = RunPodClient::new(key, store.clone());
        let trust = self.trust()?;
        match &self.selection {
            Some(selection) => RunPodInteractiveWorkerProvider::new_with_network_volume(
                client,
                profile.clone(),
                trust,
                &self.request,
                selection,
            )
            .map_err(|_| BindingError::InvalidBinding),
            None => Ok(RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust)),
        }
    }

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
                .load_remote_network_volume_selection(expected)
                .map_err(RemoteWorkspaceStopError::from)?
                != self.selection
        {
            return Err(RemoteWorkspaceStopError::StateChanged.into());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn stop_with(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    expected: &RemoteEnvironmentSummary,
    credential: impl FnOnce() -> Result<RunPodApiKey, ConfiguredRunPodStopError>,
    stop: impl FnOnce(
        &RunPodInteractiveWorkerProvider,
        &StoredRemoteAllocation,
    ) -> Result<StoredRemoteAllocation, RemoteWorkspaceStopError>,
) -> Result<RemoteEnvironmentSummary, ConfiguredRunPodStopError> {
    let admitted = RetainedRunPod::load(store, profile, expected)?;
    let runtime = admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(ConfiguredRunPodStopError::InvalidBinding)?;
    if runtime.phase.stop_requested_at_millis().is_some() {
        return Err(ConfiguredRunPodStopError::ExistingStopIntent);
    }
    if admitted.selection.is_none() {
        return Err(ConfiguredRunPodStopError::InvalidBinding);
    }
    let key = credential();
    admitted.check_current(store, &admitted.allocation)?;
    let provider = admitted.provider(store, profile, &key?)?;
    let result = stop(&provider, &admitted.allocation)?;
    admitted.check_current(store, &result)?;
    Ok(result.workspace().environment_summary())
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredRunPodStopError {
    #[error("configured RunPod Stop is supported only for retained persistent workers on Linux")]
    UnsupportedProvider,
    #[error("RunPod Stop requires a valid RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error("the configured profile or retained worker, public pin and saved HPS binding is invalid")]
    InvalidBinding,
    #[error("Stop intent already exists; use Check saved Stop without sending another Stop request")]
    ExistingStopIntent,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error("RunPod Stop could not be verified; refresh saved state and use Check if Stop intent exists")]
    Stop(#[from] RemoteWorkspaceStopError),
}

#[cfg(target_os = "linux")]
impl From<BindingError> for ConfiguredRunPodStopError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable => Self::CredentialUnavailable,
            BindingError::InvalidBinding => Self::InvalidBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => Self::Stop(error),
        }
    }
}
