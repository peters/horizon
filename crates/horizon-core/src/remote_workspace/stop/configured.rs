//! Explicit selected-environment admission into the durable Stop coordinator.

use super::{RemoteWorkspaceStopError, stop_remote_workspace};
use crate::{
    cloud_run::{CloudProvider, CloudWorkflowStore, local_docker::LocalDockerInteractiveWorkerProvider},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

/// Stop one explicitly confirmed selection using its exact named local provider profile.
/// A saved summary alone is not user authorization: call only after explicit Stop
/// confirmation, off the render thread. Closing a view or client never invokes this.
/// Reloads the owned record and rejects any changed selection before recording intent.
/// Only persistent execution is admitted; timed cleanup retains its separate policy.
/// No ambient provider, private key, allocation, restart, deletion or fallback is used.
/// Returns only safe overview metadata; saved completion is not a live observation,
/// a task checkpoint, or a guarantee of preserved process memory.
/// # Errors
/// Rejects unsupported/unconfigured providers, timed execution, stale or foreign selections, and
/// unverified Stop outcomes. Provider failures can leave durable Stop intent: refresh
/// the saved inventory before an explicit retry. Diagnostics redact private payloads.
pub fn stop_configured_remote_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentSummary, ConfiguredStopError> {
    if expected.provider != CloudProvider::LocalDocker {
        return Err(ConfiguredStopError::UnsupportedProvider);
    }
    let profile = config.local_docker_profile(&expected.profile)?;
    let current = store
        .load_remote_workspace(&expected.owning_session_id, &expected.workspace_local_id)
        .map_err(RemoteWorkspaceStopError::from)?
        .ok_or(RemoteWorkspaceStopError::MissingAllocation)?;
    if current.environment_summary() != *expected {
        return Err(RemoteWorkspaceStopError::StateChanged.into());
    }
    let provider = LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone())
        .map_err(|_| RemoteWorkspaceStopError::ProviderUnavailable)?;
    Ok(stop_remote_workspace(store, &provider, &current)?
        .workspace()
        .environment_summary())
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredStopError {
    #[error("explicit data-retaining Stop is not supported for this environment's provider")]
    UnsupportedProvider,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Stop(#[from] RemoteWorkspaceStopError),
}
