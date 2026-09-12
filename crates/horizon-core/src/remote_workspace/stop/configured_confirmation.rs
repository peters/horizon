//! Explicit configured observation of saved Stop intent; only verified completion changes local state.

use super::RemoteWorkspaceStopError;
use crate::{
    cloud_run::{CloudWorkflowStore, interactive_worker_stop::InteractiveWorkerStopObservation},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

#[cfg(target_os = "linux")]
use {
    super::{RemoteWorkspaceStopConfirmation, confirm_remote_workspace_stop, current_millis},
    crate::cloud_run::{
        CloudProvider, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::InteractiveWorkerRequest,
        runpod::{
            RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider,
            RunPodNetworkVolumeExpectation, RunPodProfile, validate_target,
        },
    },
};

/// Safe overview result, not task, filesystem, billing or current SSH readiness proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredStopConfirmation {
    pub saved: RemoteEnvironmentSummary,
    pub observation: InteractiveWorkerStopObservation,
}

/// Check one existing Stop intent using its exact named profile, worker and saved public pin.
/// Currently supports `RunPod` on Linux. Run off the render thread after an explicit check.
/// Provider operations are read-only, but retained-stopped proof may CAS-write local completion.
/// Pending, absence and errors retain identity and intent; saved Stopped timestamps are not renewed.
/// No private SSH key, first-pin lookup, Stop replay, setup, creation or deletion is attempted.
/// # Errors
/// Rejects stale or malformed ownership, missing intent/trust, incompatible profiles/storage,
/// unavailable credentials, provider failures and conflicting local updates, with redacted errors.
pub fn confirm_configured_remote_environment_stop(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredStopConfirmation, ConfiguredStopConfirmationError> {
    #[cfg(target_os = "linux")]
    {
        if expected.provider != CloudProvider::RunPod {
            return Err(ConfiguredStopConfirmationError::UnsupportedProvider);
        }
        runpod_with(
            store,
            config.runpod_profile(&expected.profile)?,
            expected,
            || RunPodApiKey::from_env().map_err(|_| ConfiguredStopConfirmationError::CredentialUnavailable),
            |provider, allocation| confirm_remote_workspace_stop(store, provider, allocation),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, expected);
        Err(ConfiguredStopConfirmationError::UnsupportedProvider)
    }
}

#[cfg(target_os = "linux")]
struct Admission {
    allocation: StoredRemoteAllocation,
    request: InteractiveWorkerRequest,
    trust: RunPodHostTrust,
    selection: Option<RunPodNetworkVolumeExpectation>,
}

#[cfg(target_os = "linux")]
fn admit(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    expected: &RemoteEnvironmentSummary,
) -> Result<Admission, ConfiguredStopConfirmationError> {
    use ConfiguredStopConfirmationError::InvalidBinding;
    let allocation = store
        .load_remote_allocation(&expected.owning_session_id, &expected.workspace_local_id)
        .map_err(RemoteWorkspaceStopError::from)?
        .ok_or(RemoteWorkspaceStopError::MissingAllocation)?;
    if allocation.workspace().environment_summary() != *expected {
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    let request = allocation.worker_request().map_err(RemoteWorkspaceStopError::from)?;
    if !request.is_valid_for(CloudProvider::RunPod) {
        return Err(InvalidBinding);
    }
    validate_target(&request.target, profile).map_err(|_| InvalidBinding)?;
    let runtime = allocation.workspace().state().runtime.as_ref().ok_or(InvalidBinding)?;
    let requested = runtime
        .phase
        .stop_requested_at_millis()
        .ok_or(RemoteWorkspaceStopError::MissingStopIntent)?;
    if runtime.cleanup.is_some() {
        return Err(RemoteWorkspaceStopError::ManagementConflict.into());
    }
    if request.target.lifetime != WorkerLifetime::Persistent {
        return Err(RemoteWorkspaceStopError::UnsupportedLifetime.into());
    }
    if requested > current_millis()? {
        return Err(RemoteWorkspaceStopError::InvalidTimestamp.into());
    }
    let worker = runtime.worker.as_ref().ok_or(RemoteWorkspaceStopError::MissingWorker)?;
    let ssh = runtime.ssh.as_ref().ok_or(RemoteWorkspaceStopError::MissingTrust)?;
    if worker.identity.workflow_id != request.workflow_id
        || worker.identity.job_id != request.job_id
        || worker.target != request.target
        || worker.ssh_public_key != request.ssh_public_key
    {
        return Err(InvalidBinding);
    }
    let trust = RunPodHostTrust::retained(worker, ssh).map_err(|_| InvalidBinding)?;
    let selection = store
        .load_remote_network_volume_selection(&allocation)
        .map_err(RemoteWorkspaceStopError::from)?;
    if let Some(selection) = &selection {
        selection.validate().map_err(|_| InvalidBinding)?;
        if profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &selection.data_center_id)
        {
            return Err(InvalidBinding);
        }
    } else if profile.volume_gib == 0 {
        return Err(InvalidBinding);
    }
    Ok(Admission {
        allocation,
        request,
        trust,
        selection,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn runpod_with(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    expected: &RemoteEnvironmentSummary,
    credential: impl FnOnce() -> Result<RunPodApiKey, ConfiguredStopConfirmationError>,
    confirm: impl FnOnce(
        &RunPodInteractiveWorkerProvider,
        &StoredRemoteAllocation,
    ) -> Result<RemoteWorkspaceStopConfirmation, RemoteWorkspaceStopError>,
) -> Result<ConfiguredStopConfirmation, ConfiguredStopConfirmationError> {
    let admitted = admit(store, profile, expected)?;
    let key = credential();
    check_current(store, &admitted.allocation, admitted.selection.as_ref())?;
    let client = RunPodClient::new(&key?, store.clone());
    let provider = match &admitted.selection {
        Some(selection) => RunPodInteractiveWorkerProvider::new_with_network_volume(
            client,
            profile.clone(),
            admitted.trust,
            &admitted.request,
            selection,
        )
        .map_err(|_| ConfiguredStopConfirmationError::InvalidBinding)?,
        None => RunPodInteractiveWorkerProvider::new(client, profile.clone(), admitted.trust),
    };
    let result = confirm(&provider, &admitted.allocation);
    let current = result
        .as_ref()
        .map_or(&admitted.allocation, |result| &result.allocation);
    check_current(store, current, admitted.selection.as_ref())?;
    let result = result?;
    Ok(ConfiguredStopConfirmation {
        saved: result.allocation.workspace().environment_summary(),
        observation: result.observation,
    })
}

#[cfg(target_os = "linux")]
fn check_current(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteAllocation,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<(), ConfiguredStopConfirmationError> {
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
            .as_ref()
            != selection
    {
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredStopConfirmationError {
    #[error("saved Stop checks are not supported for this environment's provider on this platform")]
    UnsupportedProvider,
    #[error("saved Stop checks require a valid RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error("the configured profile or retained public worker, pin and storage binding is invalid")]
    InvalidBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Stop(#[from] RemoteWorkspaceStopError),
}
