//! Configured Azure Start: the same admission as the Azure Stop paths (named profile,
//! immutable CPU profile binding, retained worker with its complete handle and public
//! pin, no `RunPod` storage expectation), a saved Stopped record or existing Start
//! intent, the lazy CLI credential and client only after admission, the binding
//! rechecked around the one provider start, and the shared Start coordinator.

use super::{RemoteWorkspaceStart, RemoteWorkspaceStartError, start_remote_workspace};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, StoredRemoteAllocation, azure::AzureProfile,
        interactive_worker::InteractiveWorkerLifecycle, interactive_worker_start::InteractiveWorkerStartProvider,
    },
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::{
        RemoteEnvironmentSummary, RemoteRuntimePhase,
        stop::{
            ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError,
            configured_azure::{Bound, RetainedAzure},
        },
    },
};

/// Safe overview result of one start: the saved record after the renewed observation
/// (`Reconciling`), the provider's lifecycle, and whether the worker already ran.
/// Reconnecting session panels re-establishes readiness; nothing resumed a task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredAzureStart {
    pub saved: RemoteEnvironmentSummary,
    pub lifecycle: InteractiveWorkerLifecycle,
    pub already_running: bool,
}

/// Start the retained compute of one explicitly confirmed, saved-Stopped Azure worker.
/// Run off the render thread after explicit confirmation. Requires the named
/// `remote.azure` profile, the allocation's immutable CPU profile binding, the retained
/// worker with its complete handle and complete saved public pin, and no `RunPod` storage
/// expectation, all before the subscription-pinned CLI credential or client exists.
/// Durable Start intent precedes the provider call through the shared coordinator; the
/// same entry point retries an existing intent and re-posts nothing that already runs.
/// Only the exact saved worker under its saved pin is accepted; a replacement pin is
/// never adopted. Does not create, delete, prepare a repository, deliver credentials
/// or replay a task; compute billing resumes at the profile's declared cost.
/// # Errors
/// Rejects unsupported, stale, missing or malformed selections, profile, binding, pin
/// or storage drift, records without a saved Stop, competing management intent,
/// unverified starts, absence and identity drift. Failures after dispatch retain
/// intent and identity for an explicit retry.
pub fn start_configured_azure_environment(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredAzureStart, ConfiguredAzureStartError> {
    if expected.provider != CloudProvider::Azure {
        return Err(ConfiguredAzureStartError::UnsupportedProvider);
    }
    let profile = config.azure_profile(&expected.profile)?;
    start_with(
        store,
        profile,
        expected,
        |_admitted| RetainedAzure::client(store, profile),
        |provider, allocation| start_remote_workspace(store, provider, allocation),
    )
}

/// Admission, then the provider, then the shared Start coordinator. A record without a
/// saved Stop or Start intent is refused before the client exists; the saved state is
/// rechecked after client construction; the binding and storage are rechecked inside
/// the provider call, after intent was recorded and before the renewed observation is
/// written; nothing fallible follows that write. `client` and `start` are injectable
/// so tests run the real ordering without the Azure CLI or ARM.
pub(super) fn start_with<P: InteractiveWorkerStartProvider>(
    store: &CloudWorkflowStore,
    profile: &AzureProfile,
    expected: &RemoteEnvironmentSummary,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    start: impl FnOnce(&Bound<'_, P>, &StoredRemoteAllocation) -> Result<RemoteWorkspaceStart, RemoteWorkspaceStartError>,
) -> Result<ConfiguredAzureStart, ConfiguredAzureStartError> {
    let admitted = RetainedAzure::load(store, profile, expected)?;
    let phase = admitted
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(BindingError::InvalidBinding)?
        .phase;
    if !matches!(
        phase,
        RemoteRuntimePhase::Stopped { .. } | RemoteRuntimePhase::Starting { .. }
    ) {
        return Err(RemoteWorkspaceStartError::NotStopped.into());
    }
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound::new(provider?, store, &admitted);
    let result = start(&bound, &admitted.allocation);
    if bound.drifted() {
        // Intent is recorded and compute may have been started; the renewed
        // observation was not written. An explicit retry resolves it.
        return Err(RemoteWorkspaceStartError::StateChanged.into());
    }
    let result = result?;
    Ok(ConfiguredAzureStart {
        saved: result.allocation.workspace().environment_summary(),
        lifecycle: result.lifecycle,
        already_running: result.already_running,
    })
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredAzureStartError {
    #[error("configured Azure Start is supported only for saved-Stopped persistent Azure workers")]
    UnsupportedProvider,
    #[error(
        "the configured Azure profile or retained worker, public pin, profile binding and storage binding is invalid"
    )]
    InvalidBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error("Azure Start could not be verified; refresh saved state and retry Start explicitly")]
    Start(#[from] RemoteWorkspaceStartError),
}

impl From<BindingError> for ConfiguredAzureStartError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            // The Azure credential is lazy and never consulted during admission.
            BindingError::CredentialUnavailable | BindingError::InvalidBinding => Self::InvalidBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => Self::Start(match error {
                RemoteWorkspaceStopError::MissingAllocation => RemoteWorkspaceStartError::MissingAllocation,
                RemoteWorkspaceStopError::MissingWorker => RemoteWorkspaceStartError::MissingWorker,
                RemoteWorkspaceStopError::MissingTrust => RemoteWorkspaceStartError::MissingTrust,
                RemoteWorkspaceStopError::StateChanged => RemoteWorkspaceStartError::StateChanged,
                RemoteWorkspaceStopError::ProviderMismatch => RemoteWorkspaceStartError::ProviderMismatch,
                RemoteWorkspaceStopError::UnsupportedLifetime => RemoteWorkspaceStartError::UnsupportedLifetime,
                RemoteWorkspaceStopError::ManagementConflict => RemoteWorkspaceStartError::ManagementConflict,
                RemoteWorkspaceStopError::InvalidTimestamp => RemoteWorkspaceStartError::InvalidTimestamp,
                RemoteWorkspaceStopError::StorageUnavailable
                | RemoteWorkspaceStopError::MissingStopIntent
                | RemoteWorkspaceStopError::ProviderUnavailable
                | RemoteWorkspaceStopError::ResourceAbsent => RemoteWorkspaceStartError::StorageUnavailable,
            }),
        }
    }
}
