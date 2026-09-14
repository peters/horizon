//! Join an overview selection to its exact saved profile and owned allocation.

use super::{RemoteEnvironmentObservation, RemoteEnvironmentObservationError, observe_remote_environment};
use crate::{
    cloud_run::{CloudProvider, CloudWorkflowStore, local_docker::LocalDockerInteractiveWorkerProvider},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::{
        RemoteEnvironmentSummary,
        stop::{ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError},
    },
};

/// Check one saved selection using only its explicitly configured provider profile.
/// This synchronous read belongs off the render thread. It never allocates, adopts,
/// manages or attaches a worker, and does not need the retained private SSH key.
/// On Linux, `RunPod` and Azure checks require an already retained worker and complete
/// host pin; Azure additionally requires the allocation's immutable profile binding.
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
    if expected.provider == CloudProvider::Azure {
        return observe_azure(store, config, expected);
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

/// Azure checks share the Stop and Start admission (named `remote.azure` profile,
/// immutable CPU profile binding, retained worker with its complete handle and saved
/// pin, no `RunPod` storage expectation) with one difference: pending management
/// intent stays observable, because this path never writes or manages. The
/// subscription-pinned CLI credential and client are built lazily after admission and
/// every provider read is fenced by the binding recheck; drift is a changed state.
fn observe_azure(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentObservation, ConfiguredObservationError> {
    #[cfg(target_os = "linux")]
    {
        use crate::remote_workspace::stop::configured_azure::RetainedAzure;
        let profile = config.azure_profile(&expected.profile)?;
        azure_with(
            store,
            profile,
            expected,
            |_admitted| RetainedAzure::client(store, profile),
            |provider, workspace| observe_remote_environment(store, provider, workspace),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, expected);
        Err(ConfiguredObservationError::UnsupportedProvider)
    }
}

/// Admission, then the lazy client, then the admitted snapshot rechecked, then the
/// shared read through the bound provider, then the snapshot rechecked again. `client`
/// and `observe` are injectable so tests run the real ordering without the Azure CLI or ARM.
#[cfg(target_os = "linux")]
pub(super) fn azure_with<P, T>(
    store: &CloudWorkflowStore,
    profile: &crate::cloud_run::azure::AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&crate::remote_workspace::stop::configured_azure::RetainedAzure) -> Result<P, BindingError>,
    observe: impl FnOnce(
        &crate::remote_workspace::stop::configured_azure::Bound<'_, P>,
        &crate::cloud_run::StoredRemoteWorkspace,
    ) -> Result<T, RemoteEnvironmentObservationError>,
) -> Result<T, ConfiguredObservationError>
where
    P: crate::cloud_run::interactive_worker::InteractiveWorkerProvider,
{
    use crate::remote_workspace::stop::configured_azure::{Bound, RetainedAzure};
    let admitted = RetainedAzure::load_observable(store, profile, expected)?;
    // The client (and its lazy credential) is built first, then the saved state is
    // rechecked, so drift during that step is a changed state even when the client
    // could not be built.
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound::new(provider?, store, &admitted);
    let result = observe(&bound, admitted.allocation.workspace());
    if bound.drifted() {
        return Err(RemoteEnvironmentObservationError::StateChanged.into());
    }
    // The read writes nothing, so the whole admitted snapshot must still hold after it.
    admitted.check_current(store, &admitted.allocation)?;
    result.map_err(Into::into)
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredObservationError {
    #[error("provider checks are not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("RunPod provider checks require a valid RUNPOD_API_KEY supplied to the controller")]
    RunPodCredentialUnavailable,
    #[error("the configured RunPod profile or retained public worker, pin and storage binding is invalid")]
    InvalidRunPodBinding,
    #[error(
        "the configured Azure profile or retained public worker, pin, profile binding and storage binding is invalid"
    )]
    InvalidAzureBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Observation(#[from] RemoteEnvironmentObservationError),
}

impl From<BindingError> for ConfiguredObservationError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable => Self::RunPodCredentialUnavailable,
            BindingError::InvalidBinding => Self::InvalidAzureBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => match error {
                RemoteWorkspaceStopError::MissingAllocation => {
                    RemoteEnvironmentObservationError::MissingAllocation.into()
                }
                RemoteWorkspaceStopError::ProviderMismatch => {
                    RemoteEnvironmentObservationError::ProviderMismatch.into()
                }
                RemoteWorkspaceStopError::StateChanged => RemoteEnvironmentObservationError::StateChanged.into(),
                RemoteWorkspaceStopError::StorageUnavailable => {
                    RemoteEnvironmentObservationError::StorageUnavailable.into()
                }
                // A missing retained worker or pin, a timed target, or a Stop-only outcome
                // is an invalid binding for a read that requires the retained public identity.
                RemoteWorkspaceStopError::MissingWorker
                | RemoteWorkspaceStopError::MissingTrust
                | RemoteWorkspaceStopError::UnsupportedLifetime
                | RemoteWorkspaceStopError::ManagementConflict
                | RemoteWorkspaceStopError::MissingStopIntent
                | RemoteWorkspaceStopError::InvalidTimestamp
                | RemoteWorkspaceStopError::ProviderUnavailable
                | RemoteWorkspaceStopError::ResourceAbsent => Self::InvalidAzureBinding,
            },
        }
    }
}
