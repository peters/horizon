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
/// # Errors
/// Rejects unsupported/unconfigured providers, stale selections and unsafe or failed
/// observations. Diagnostics do not include provider output or executable task data.
pub fn observe_configured_remote_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentObservation, ConfiguredObservationError> {
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

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredObservationError {
    #[error("provider checks are not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Observation(#[from] RemoteEnvironmentObservationError),
}
