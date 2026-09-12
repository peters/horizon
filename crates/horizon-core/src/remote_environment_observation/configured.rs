//! Join an overview selection to its exact saved profile and owned allocation.

use super::{RemoteEnvironmentObservation, RemoteEnvironmentObservationError, observe_remote_environment};
use crate::{
    cloud_run::{CloudProvider, CloudWorkflowStore, local_docker::LocalDockerInteractiveWorkerProvider},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

/// Check one saved selection using only its explicitly configured provider profile.
/// This synchronous read belongs off the render thread. It never allocates, adopts,
/// manages or attaches a worker, and does not need the retained private SSH key.
/// On Linux, `RunPod` checks require an already retained worker and complete host pin.
/// # Errors
/// Rejects unsupported/unconfigured providers, stale selections and unsafe or failed
/// observations. Diagnostics do not include provider output or executable task data.
pub fn observe_configured_remote_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentObservation, ConfiguredObservationError> {
    if expected.provider == CloudProvider::RunPod {
        return observe_runpod(store, config, expected);
    }
    if expected.provider != CloudProvider::LocalDocker {
        return Err(ConfiguredObservationError::UnsupportedProvider);
    }
    let profile = config.local_docker_profile(&expected.profile)?;
    let current = store
        .load_remote_workspace(&expected.owning_session_id, &expected.workspace_local_id)
        .map_err(RemoteEnvironmentObservationError::from)?
        .ok_or(RemoteEnvironmentObservationError::MissingAllocation)?;
    if current.environment_summary() != *expected {
        return Err(RemoteEnvironmentObservationError::StateChanged.into());
    }
    let provider = LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone())
        .map_err(|_| RemoteEnvironmentObservationError::ProviderUnavailable)?;
    Ok(observe_remote_environment(store, &provider, &current)?)
}

fn observe_runpod(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentObservation, ConfiguredObservationError> {
    #[cfg(target_os = "linux")]
    {
        runpod_with(
            store,
            config.runpod_profile(&expected.profile)?,
            expected,
            || {
                crate::cloud_run::runpod::RunPodApiKey::from_env()
                    .map_err(|_| ConfiguredObservationError::RunPodCredentialUnavailable)
            },
            |provider, workspace| observe_remote_environment(store, provider, workspace),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, expected);
        Err(ConfiguredObservationError::UnsupportedProvider)
    }
}

/// Retained public identity only: no private key, first-pin bootstrap or recovery write.
#[cfg(target_os = "linux")]
pub(super) fn runpod_with<T>(
    store: &CloudWorkflowStore,
    profile: &crate::cloud_run::runpod::RunPodProfile,
    expected: &RemoteEnvironmentSummary,
    credential: impl FnOnce() -> Result<crate::cloud_run::runpod::RunPodApiKey, ConfiguredObservationError>,
    observe: impl FnOnce(
        &crate::cloud_run::runpod::RunPodInteractiveWorkerProvider,
        &crate::cloud_run::StoredRemoteWorkspace,
    ) -> Result<T, RemoteEnvironmentObservationError>,
) -> Result<T, ConfiguredObservationError> {
    use crate::cloud_run::runpod::{RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider, validate_target};
    let current = store
        .load_remote_workspace(&expected.owning_session_id, &expected.workspace_local_id)
        .map_err(RemoteEnvironmentObservationError::from)?
        .ok_or(RemoteEnvironmentObservationError::MissingAllocation)?;
    if current.environment_summary() != *expected {
        return Err(RemoteEnvironmentObservationError::StateChanged.into());
    }
    let allocation = super::load_current(store, &current)?;
    let request = allocation
        .worker_request()
        .map_err(RemoteEnvironmentObservationError::from)?;
    if !request.is_valid_for(CloudProvider::RunPod) {
        return Err(ConfiguredObservationError::InvalidRunPodBinding);
    }
    validate_target(&request.target, profile).map_err(|_| ConfiguredObservationError::InvalidRunPodBinding)?;
    let runtime = current
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteEnvironmentObservationError::MissingAllocation)?;
    let (worker, ssh) = runtime
        .worker
        .as_ref()
        .zip(runtime.ssh.as_ref())
        .ok_or(ConfiguredObservationError::InvalidRunPodBinding)?;
    if worker.identity.workflow_id != request.workflow_id
        || worker.identity.job_id != request.job_id
        || worker.target != request.target
        || worker.ssh_public_key != request.ssh_public_key
    {
        return Err(ConfiguredObservationError::InvalidRunPodBinding);
    }
    let trust = RunPodHostTrust::retained(worker, ssh).map_err(|_| ConfiguredObservationError::InvalidRunPodBinding)?;
    let selection = store
        .load_remote_network_volume_selection(&allocation)
        .map_err(RemoteEnvironmentObservationError::from)?;
    if let Some(selection) = &selection {
        selection
            .validate()
            .map_err(|_| ConfiguredObservationError::InvalidRunPodBinding)?;
        if profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &selection.data_center_id)
        {
            return Err(ConfiguredObservationError::InvalidRunPodBinding);
        }
    }
    let check_current = || -> Result<(), ConfiguredObservationError> {
        let current = super::load_current(store, allocation.workspace())?;
        if current != allocation
            || store
                .load_remote_network_volume_selection(&current)
                .map_err(RemoteEnvironmentObservationError::from)?
                != selection
        {
            return Err(RemoteEnvironmentObservationError::StateChanged.into());
        }
        Ok(())
    };
    let key = credential()?;
    check_current()?;
    let client = RunPodClient::new(&key, store.clone());
    let provider = match &selection {
        Some(selection) => RunPodInteractiveWorkerProvider::new_with_network_volume(
            client,
            profile.clone(),
            trust,
            &request,
            selection,
        )
        .map_err(|_| ConfiguredObservationError::InvalidRunPodBinding)?,
        None => RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust),
    };
    let result = observe(&provider, allocation.workspace())?;
    check_current()?;
    Ok(result)
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredObservationError {
    #[error("provider checks are not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("RunPod provider checks require a valid RUNPOD_API_KEY supplied to the controller")]
    RunPodCredentialUnavailable,
    #[error("the configured RunPod profile or retained public worker, pin and storage binding is invalid")]
    InvalidRunPodBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Observation(#[from] RemoteEnvironmentObservationError),
}
