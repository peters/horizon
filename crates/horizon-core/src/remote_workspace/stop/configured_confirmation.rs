//! Explicit configured observation of saved Stop intent; only verified completion changes local state.

use super::RemoteWorkspaceStopError;
use crate::{
    cloud_run::{CloudWorkflowStore, interactive_worker_stop::InteractiveWorkerStopObservation},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

#[cfg(target_os = "linux")]
use {
    super::{
        RemoteWorkspaceStopConfirmation, configured_runpod::RetainedRunPod, confirm_remote_workspace_stop,
        current_millis,
    },
    crate::cloud_run::{
        CloudProvider, StoredRemoteAllocation,
        runpod::{RunPodApiKey, RunPodInteractiveWorkerProvider, RunPodProfile},
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
    let admitted = RetainedRunPod::load(store, profile, expected)?;
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
    let key = credential();
    admitted.check_current(store, &admitted.allocation)?;
    let provider = admitted.provider(store, profile, &key?)?;
    let result = confirm(&provider, &admitted.allocation);
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
