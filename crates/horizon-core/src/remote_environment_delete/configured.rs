//! Named-provider admission for explicit deletion, not SSH or provisioning authority.

mod admission;
mod azure;
mod runpod;

use super::RemoteEnvironmentDeleteError;
use crate::{
    cloud_run::{CloudProvider, CloudStoreError, CloudWorkflowStore, RemoteWorkspaceStoreError},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

/// Saved deletion state. A completed tombstone records historical, not live, absence.
#[derive(Debug, Eq, PartialEq)]
pub struct ConfiguredEnvironmentDeletion {
    pub saved: RemoteEnvironmentSummary,
    pub absence_verified: bool,
}

/// Send one explicitly confirmed Delete for the exact configured persistent worker.
///
/// The caller must disclose running-task and unpushed-data loss and complete provider
/// scope: the owned Azure group or the `RunPod` Pod, never its independent network volume.
/// Requires exact saved ownership, request and storage/profile bindings, but no saved
/// SSH pin, private key or reachable guest. Admission precedes lazy credentials; the
/// shared coordinator saves intent before dispatch. Run off the render thread.
/// `RunPod` additionally requires an owned-Present preflight with this same client;
/// completion also requires acknowledged Delete and a subsequent absence observation.
/// An unbound 404 cannot establish absence or authorize a deletion request.
/// # Errors
/// Rejects stale or unsupported state, missing bindings, competing intent and missing
/// credentials. Unverified dispatch retains intent; Check never resends Delete.
pub fn delete_configured_remote_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredEnvironmentDeletion, ConfiguredEnvironmentDeleteError> {
    execute(store, config, expected, Operation::Delete)
}

/// Observe saved Delete intent without sending Delete or contacting the guest.
/// Completed tombstones return without constructing a credential or provider.
/// Without durable account binding, a fresh `RunPod` Check can report Present but
/// cannot confirm bare absence. Lost-reply/restart completion remains unverified.
/// # Errors
/// Rejects stale, unbound or unconfigured state and failed exact absence observations.
pub fn confirm_configured_remote_environment_deletion(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredEnvironmentDeletion, ConfiguredEnvironmentDeleteError> {
    execute(store, config, expected, Operation::Check)
}

/// Retry pending Delete only after a separate explicit destructive confirmation.
/// The earlier request may still be in progress. The coordinator observes first;
/// only a surviving resource permits one CAS-guarded Delete and another observation.
/// `RunPod` absence requires owned presence and acknowledged Delete in this operation;
/// a resource already absent at a fresh retry remains unverified.
/// Never invoke this from refresh, reconnect, view closure or automatic cleanup.
/// # Errors
/// Rejects missing pending intent, stale bindings and unsuccessful observation.
pub fn retry_configured_remote_environment_deletion(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredEnvironmentDeletion, ConfiguredEnvironmentDeleteError> {
    execute(store, config, expected, Operation::Retry)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Operation {
    Delete,
    Check,
    Retry,
}

fn execute(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
    operation: Operation,
) -> Result<ConfiguredEnvironmentDeletion, Error> {
    match expected.provider {
        CloudProvider::RunPod => admission::with_provider(store, config, expected, operation, |admitted| {
            runpod::provider(store, config.runpod_profile(&expected.profile)?, admitted)
        }),
        CloudProvider::Azure => admission::with_provider(store, config, expected, operation, |_admitted| {
            azure::provider(store, config.azure_profile(&expected.profile)?)
        }),
        CloudProvider::LocalDocker => Err(Error::UnsupportedProvider),
    }
}

type Error = ConfiguredEnvironmentDeleteError;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredEnvironmentDeleteError {
    #[error("configured deletion supports retained persistent RunPod or Azure workers")]
    UnsupportedProvider,
    #[error("the named profile, retained worker or immutable storage/profile binding is invalid")]
    InvalidBinding,
    #[error("RUNPOD_API_KEY is unavailable; no Delete was sent")]
    CredentialUnavailable,
    #[error(
        "RunPod absence requires owned presence and acknowledged Delete in this operation; no completion was recorded"
    )]
    UnverifiedRunPodContext,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Delete(#[from] RemoteEnvironmentDeleteError),
}

impl From<CloudStoreError> for Error {
    fn from(error: CloudStoreError) -> Self {
        Self::Delete(error.into())
    }
}

impl From<RemoteWorkspaceStoreError> for Error {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        Self::Delete(error.into())
    }
}

#[cfg(test)]
mod tests;
