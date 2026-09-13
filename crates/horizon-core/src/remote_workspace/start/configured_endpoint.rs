//! Configured admission for an explicit authenticated saved-connection refresh.

use super::RemoteEndpointRefreshError as RefreshError;
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::RemoteEnvironmentSummary,
};

#[cfg(target_os = "linux")]
use {
    super::{endpoint::refresh_phase_allowed, refresh_remote_worker_endpoint},
    crate::{
        cloud_run::{
            CloudProvider, StoredRemoteAllocation,
            interactive_worker::InteractiveWorker,
            runpod::{RunPodApiKey, RunPodProfile},
        },
        remote_workspace::stop::{
            ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError as StopError,
            configured_runpod::RetainedRunPod,
        },
    },
};

/// Saved summary after original-key authentication; not compute Start or task readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredEndpointRefresh {
    pub saved: RemoteEnvironmentSummary,
}

/// Refresh only an explicitly selected Linux `RunPod` worker's saved connection.
/// Requires its exact retained profile, HPS selection, worker and public pin before
/// recovering the original private identity and then lazily reading `RUNPOD_API_KEY`.
/// The shared coordinator revalidates that identity for its original-key proof and
/// independent provider re-observation. Only connection coordinates may change;
/// saved phase and Start intent remain. Run off the render thread after consent.
/// Does not start compute, reconnect panels, replay tasks, discover or generate keys,
/// establish first trust, mutate the provider or certify storage durability.
/// # Errors
/// Refuses unsupported platforms/providers, invalid or stale bindings, missing
/// original identity, unavailable credentials, failed proof and competing updates.
pub fn refresh_configured_runpod_connection(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredEndpointRefresh, ConfiguredEndpointRefreshError> {
    #[cfg(target_os = "linux")]
    {
        if expected.provider != CloudProvider::RunPod {
            return Err(Error::UnsupportedProvider);
        }
        let profile = config.runpod_profile(&expected.profile)?;
        refresh_with(
            store,
            profile,
            expected,
            |worker| {
                identities
                    .recover(
                        worker.identity.workflow_id,
                        worker.identity.job_id,
                        &worker.ssh_public_key,
                    )
                    .map(|_| ())
                    .map_err(|_| RefreshError::IdentityUnavailable.into())
            },
            |admitted| {
                let key = RunPodApiKey::from_env().map_err(|_| Error::CredentialUnavailable)?;
                admitted.provider(store, profile, &key).map_err(Into::into)
            },
            |provider, allocation| refresh_remote_worker_endpoint(store, identities, provider, allocation),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, expected);
        Err(Error::UnsupportedProvider)
    }
}

#[cfg(target_os = "linux")]
fn refresh_with<P>(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    expected: &RemoteEnvironmentSummary,
    recover: impl FnOnce(&InteractiveWorker) -> Result<(), Error>,
    client: impl FnOnce(&RetainedRunPod) -> Result<P, Error>,
    refresh: impl FnOnce(&P, &StoredRemoteAllocation) -> Result<StoredRemoteAllocation, RefreshError>,
) -> Result<ConfiguredEndpointRefresh, Error> {
    let admitted = RetainedRunPod::load(store, profile, expected)?;
    if admitted.selection.is_none() {
        return Err(Error::InvalidBinding);
    }
    let state = admitted.allocation.workspace().state();
    if !refresh_phase_allowed(state) {
        return Err(RefreshError::ManagementConflict.into());
    }
    let worker = state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref())
        .ok_or(RefreshError::IdentityUnavailable)?;
    let identity = recover(worker);
    admitted.check_current(store, &admitted.allocation)?;
    identity?;
    let provider = client(&admitted);
    // A failed credential lookup must not hide a concurrent allocation/HPS change.
    admitted.check_current(store, &admitted.allocation)?;
    let result = refresh(&provider?, &admitted.allocation);
    admitted.check_current(store, result.as_ref().unwrap_or(&admitted.allocation))?;
    checked_summary(expected, result?.workspace().environment_summary())
}

#[cfg(target_os = "linux")]
fn checked_summary(
    expected: &RemoteEnvironmentSummary,
    saved: RemoteEnvironmentSummary,
) -> Result<ConfiguredEndpointRefresh, Error> {
    if saved.revision != expected.revision && expected.revision.checked_add(1) != Some(saved.revision) {
        return Err(RefreshError::StateChanged.into());
    }
    let mut unchanged = saved.clone();
    unchanged.revision = expected.revision;
    if unchanged != *expected {
        return Err(RefreshError::StateChanged.into());
    }
    Ok(ConfiguredEndpointRefresh { saved })
}

type Error = ConfiguredEndpointRefreshError;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredEndpointRefreshError {
    #[error("configured connection refresh requires a retained persistent RunPod worker on Linux")]
    UnsupportedProvider,
    #[error("connection refresh requires a valid RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error("connection refresh requires the exact named profile, retained worker, public pin and saved HPS binding")]
    InvalidBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Refresh(#[from] RefreshError),
}

#[cfg(target_os = "linux")]
impl From<BindingError> for Error {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable => Self::CredentialUnavailable,
            BindingError::InvalidBinding => Self::InvalidBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => Self::Refresh(match error {
                StopError::StateChanged | StopError::MissingAllocation => RefreshError::StateChanged,
                StopError::MissingWorker | StopError::MissingTrust => RefreshError::IdentityUnavailable,
                StopError::ManagementConflict => RefreshError::ManagementConflict,
                StopError::ProviderMismatch | StopError::UnsupportedLifetime => RefreshError::Unsupported,
                _ => RefreshError::StorageUnavailable,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
