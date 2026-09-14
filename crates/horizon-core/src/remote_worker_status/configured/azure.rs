//! Read-only status under the retained Azure allocation, profile and SSH binding.

use super::{ConfiguredRemotePanelStatusError as Error, ConfiguredRemotePanelStatusRequest, RemotePanelObservation};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, StoredRemoteAllocation, azure::AzureProfile,
        interactive_worker::InteractiveWorkerProvider,
    },
    remote_provider_config::RemoteProviderConfig,
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::{RemotePanelStatus, RemotePanelStatusError},
    remote_workspace::{
        RemoteRuntimePhase,
        stop::{
            ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError,
            configured_azure::{Bound, RetainedAzure},
        },
    },
    remote_workspace_recovery::{RemoteWorkspaceRecoveryError, inspect_remote_allocation},
};

pub(super) fn inspect(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
) -> Result<RemotePanelObservation, Error> {
    if request.client_session_id != request.expected.owning_session_id {
        return Err(Error::ClientSessionMismatch);
    }
    let profile = config.azure_profile(&request.expected.profile)?;
    inspect_with(
        store,
        identities,
        profile,
        request,
        |_| RetainedAzure::client(store, profile),
        |provider, allocation| {
            let recovered = inspect_remote_allocation(identities, provider, allocation)?;
            Ok(super::super::inspect_remote_panel(store, &recovered, request.panel_id)?)
        },
    )
}

pub(in crate::remote_worker_status) fn inspect_with<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    profile: &AzureProfile,
    request: ConfiguredRemotePanelStatusRequest<'_>,
    client: impl FnOnce(&RetainedAzure) -> Result<P, BindingError>,
    inspect: impl FnOnce(&Bound<'_, P>, &StoredRemoteAllocation) -> Result<RemotePanelStatus, Error>,
) -> Result<RemotePanelObservation, Error> {
    if request.client_session_id != request.expected.owning_session_id {
        return Err(Error::ClientSessionMismatch);
    }
    if request.expected.provider != CloudProvider::Azure {
        return Err(Error::UnsupportedProvider);
    }
    let admitted = RetainedAzure::load(store, profile, request.expected)?;
    let state = admitted.allocation.workspace().state();
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    match runtime.phase {
        RemoteRuntimePhase::Stopping { .. } | RemoteRuntimePhase::Starting { .. } => {
            return Err(RemotePanelStatusError::ManagementPending.into());
        }
        RemoteRuntimePhase::Ready | RemoteRuntimePhase::Reconciling => {}
        _ => return Err(RemotePanelStatusError::WorkerUnavailable.into()),
    }
    if !state
        .spec
        .panels
        .iter()
        .any(|panel| panel.panel_local_id == request.panel_id)
    {
        return Err(RemotePanelStatusError::UnknownPanel.into());
    }
    let expected = admitted
        .allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    identities
        .recover(expected.workflow_id, expected.job_id, &expected.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound::new(provider?, store, &admitted);
    let result = inspect(&bound, &admitted.allocation);
    if bound.drifted() {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    // Discard even a successful status if ownership, allocation or binding changed during I/O.
    admitted.check_current(store, &admitted.allocation)?;
    super::observation(request.panel_id, result?)
}

impl From<BindingError> for Error {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable | BindingError::InvalidBinding => Self::InvalidAzureBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => match error {
                RemoteWorkspaceStopError::MissingAllocation => RemoteWorkspaceRecoveryError::MissingAllocation.into(),
                RemoteWorkspaceStopError::StateChanged => RemoteWorkspaceRecoveryError::StateChanged.into(),
                RemoteWorkspaceStopError::ProviderMismatch => RemoteWorkspaceRecoveryError::ProviderMismatch.into(),
                RemoteWorkspaceStopError::ManagementConflict => RemotePanelStatusError::ManagementPending.into(),
                RemoteWorkspaceStopError::StorageUnavailable => RemotePanelStatusError::StorageUnavailable.into(),
                RemoteWorkspaceStopError::MissingWorker
                | RemoteWorkspaceStopError::MissingTrust
                | RemoteWorkspaceStopError::UnsupportedLifetime
                | RemoteWorkspaceStopError::MissingStopIntent
                | RemoteWorkspaceStopError::InvalidTimestamp
                | RemoteWorkspaceStopError::ProviderUnavailable
                | RemoteWorkspaceStopError::ResourceAbsent => Self::InvalidAzureBinding,
            },
        }
    }
}
