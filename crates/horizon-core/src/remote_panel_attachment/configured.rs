//! Explicit current-client and exact-profile admission; no ambient provider fallback.

use super::{RemotePanelAttachError, RemotePanelAttachRequest, RemotePanelConnectionAttempt, RemotePanelTerminalSize};
use crate::{
    cloud_run::{CloudProvider, CloudWorkflowStore, local_docker::LocalDockerInteractiveWorkerProvider},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::RemotePanelStatusError,
    remote_workspace::{
        RemoteEnvironmentSummary,
        stop::{ConfiguredStopConfirmationError as BindingError, RemoteWorkspaceStopError},
    },
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

#[derive(Clone, Copy)]
pub struct ConfiguredRemotePanelAttachRequest<'a> {
    pub expected: &'a RemoteEnvironmentSummary,
    /// Actual resolved active client session, never copied from an environment or view claim.
    pub client_session_id: &'a str,
    pub panel_id: &'a str,
    pub terminal: RemotePanelTerminalSize,
}

/// Explicitly connect one existing panel through its exact configured local, `RunPod` or
/// Azure profile.
/// Run all storage, retained-key, provider and SSH work off the render thread.
/// The actual active session must own the selected environment; copied references
/// and global inventory access alone do not admit a connection. The caller must
/// additionally bind the request/result to the exact client view and config/request
/// generation. Same-owner admission does not implement cross-session Open.
/// No identity creation, task startup/replay, worker allocation, Stop, Delete or
/// provider/profile fallback occurs. Saved state is unchanged; this returns a local
/// transport attempt, not authenticated connection or workspace readiness.
/// # Errors
/// Rejects unsupported providers, stale or foreign selections, missing profiles or
/// owned allocations, and attachment failures without exposing private payloads.
pub fn attach_configured_remote_panel(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelAttachRequest<'_>,
) -> Result<RemotePanelConnectionAttempt, ConfiguredRemotePanelAttachError> {
    if !matches!(
        request.expected.provider,
        CloudProvider::LocalDocker | CloudProvider::RunPod | CloudProvider::Azure
    ) {
        return Err(ConfiguredRemotePanelAttachError::UnsupportedProvider);
    }
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemotePanelAttachError::ClientSessionMismatch);
    }
    if request.expected.provider == CloudProvider::RunPod {
        return attach_runpod(store, identities, config, request);
    }
    if request.expected.provider == CloudProvider::Azure {
        return attach_azure(store, identities, config, request);
    }
    let profile = config.local_docker_profile(&request.expected.profile)?;
    let allocation = store
        .load_remote_allocation(request.client_session_id, &request.expected.workspace_local_id)
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if allocation.workspace().environment_summary() != *request.expected {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    let provider = LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone())
        .map_err(|_| RemoteWorkspaceRecoveryError::ProviderUnavailable)?;
    Ok(super::attach_remote_panel(
        store,
        identities,
        &provider,
        RemotePanelAttachRequest {
            allocation: &allocation,
            panel_id: request.panel_id,
            terminal: request.terminal,
        },
    )?)
}

fn attach_runpod(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelAttachRequest<'_>,
) -> Result<RemotePanelConnectionAttempt, ConfiguredRemotePanelAttachError> {
    #[cfg(target_os = "linux")]
    {
        let profile = config.runpod_profile(&request.expected.profile)?;
        let allocation = store
            .load_remote_allocation(request.client_session_id, &request.expected.workspace_local_id)
            .map_err(RemoteWorkspaceRecoveryError::from)?
            .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
        if allocation.workspace().environment_summary() != *request.expected {
            return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
        }
        runpod_with(
            store,
            identities,
            profile,
            RemotePanelAttachRequest {
                allocation: &allocation,
                panel_id: request.panel_id,
                terminal: request.terminal,
            },
            || {
                crate::cloud_run::runpod::RunPodApiKey::from_env()
                    .map_err(|_| ConfiguredRemotePanelAttachError::RunPodCredentialUnavailable)
            },
            |provider, request| super::attach_remote_panel(store, identities, provider, request),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request);
        Err(RemotePanelAttachError::UnsupportedPlatform.into())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn runpod_with<T>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    profile: &crate::cloud_run::runpod::RunPodProfile,
    request: RemotePanelAttachRequest<'_>,
    credential: impl FnOnce() -> Result<crate::cloud_run::runpod::RunPodApiKey, ConfiguredRemotePanelAttachError>,
    attach: impl FnOnce(
        &crate::cloud_run::runpod::RunPodInteractiveWorkerProvider,
        RemotePanelAttachRequest<'_>,
    ) -> Result<T, RemotePanelAttachError>,
) -> Result<T, ConfiguredRemotePanelAttachError> {
    use crate::{
        PanelKind,
        cloud_run::{
            WorkerLifetime,
            runpod::{RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider, validate_target},
        },
        remote_worker_status::RemotePanelStatusError,
    };
    let allocation = request.allocation;
    super::check_current(store, allocation)?;
    let expected = allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let state = allocation.workspace().state();
    if state.spec.target.lifetime != WorkerLifetime::Persistent {
        return Err(RemotePanelAttachError::UnsupportedLifetime.into());
    }
    validate_target(&expected.target, profile).map_err(|_| ConfiguredRemotePanelAttachError::InvalidRunPodBinding)?;
    let panel = state
        .spec
        .panels
        .iter()
        .find(|panel| panel.panel_local_id == request.panel_id)
        .ok_or(RemotePanelAttachError::from(RemotePanelStatusError::UnknownPanel))?;
    if !matches!(panel.kind, PanelKind::Shell | PanelKind::Command)
        || panel.command.is_none()
        || panel.task_handoff.is_some()
        || panel.agent_session_id.is_some()
    {
        return Err(RemotePanelAttachError::from(RemotePanelStatusError::UnsupportedIntent).into());
    }
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    let (worker, ssh) = runtime
        .worker
        .as_ref()
        .zip(runtime.ssh.as_ref())
        .ok_or(RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable))?;
    let trust =
        RunPodHostTrust::retained(worker, ssh).map_err(|_| ConfiguredRemotePanelAttachError::InvalidRunPodBinding)?;
    identities
        .recover(expected.workflow_id, expected.job_id, &expected.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let selection = store
        .load_remote_network_volume_selection(allocation)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if selection.as_ref().is_some_and(|selection| {
        profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &selection.data_center_id)
    }) {
        return Err(ConfiguredRemotePanelAttachError::InvalidRunPodBinding);
    }
    // Reject locally incompatible placement before credentials; the provider also
    // enforces the complete immutable request binding before any provider I/O.
    let build = |client| match &selection {
        Some(selection) => RunPodInteractiveWorkerProvider::new_with_network_volume(
            client,
            profile.clone(),
            trust,
            &expected,
            selection,
        )
        .map_err(|_| ConfiguredRemotePanelAttachError::InvalidRunPodBinding),
        None => Ok(RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust)),
    };
    let key = credential()?;
    let provider = build(RunPodClient::new(&key, store.clone()))?;
    super::check_current(store, allocation)?;
    attach(&provider, request).map_err(Into::into)
}

/// Azure reconnection shares the Stop and Start admission: the named `remote.azure`
/// profile, the allocation's immutable CPU profile binding, the retained worker with its
/// complete handle and complete saved public pin, and no `RunPod` storage expectation,
/// all before the subscription-pinned CLI credential or client exists. A record that is
/// saved as stopping, stopped or starting is refused before the client: attachment
/// never starts compute. The client is built lazily, the saved state is rechecked after
/// it, and every provider observation inside the shared attachment is fenced by the
/// binding recheck, so drift is reported as a changed state, never as a connection.
fn attach_azure(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelAttachRequest<'_>,
) -> Result<RemotePanelConnectionAttempt, ConfiguredRemotePanelAttachError> {
    #[cfg(target_os = "linux")]
    {
        use crate::remote_workspace::stop::configured_azure::RetainedAzure;
        let profile = config.azure_profile(&request.expected.profile)?;
        azure_with(
            store,
            identities,
            profile,
            request,
            |_admitted| RetainedAzure::client(store, profile),
            |provider, request| super::attach_remote_panel(store, identities, provider, request),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request);
        Err(RemotePanelAttachError::UnsupportedPlatform.into())
    }
}

/// Admission, then the lazy client, then the shared attachment through the bound
/// provider. `client` and `attach` are injectable so tests run the real ordering
/// without the Azure CLI, ARM or SSH.
#[cfg(target_os = "linux")]
pub(super) fn azure_with<P, T>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    profile: &crate::cloud_run::azure::AzureProfile,
    request: ConfiguredRemotePanelAttachRequest<'_>,
    client: impl FnOnce(&crate::remote_workspace::stop::configured_azure::RetainedAzure) -> Result<P, BindingError>,
    attach: impl FnOnce(
        &crate::remote_workspace::stop::configured_azure::Bound<'_, P>,
        RemotePanelAttachRequest<'_>,
    ) -> Result<T, RemotePanelAttachError>,
) -> Result<T, ConfiguredRemotePanelAttachError>
where
    P: crate::cloud_run::interactive_worker::InteractiveWorkerProvider,
{
    use crate::{
        PanelKind,
        remote_workspace::{
            RemoteRuntimePhase,
            stop::configured_azure::{Bound, RetainedAzure},
        },
    };
    let admitted = RetainedAzure::load(store, profile, request.expected)?;
    let state = admitted.allocation.workspace().state();
    let runtime = state
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    // Stop and Start are separate explicit operations; a saved lifecycle phase is never
    // driven forward by a reconnection, and a stopped worker offers nothing to attach.
    match runtime.phase {
        RemoteRuntimePhase::Stopping { .. } | RemoteRuntimePhase::Starting { .. } => {
            return Err(RemotePanelAttachError::from(RemotePanelStatusError::ManagementPending).into());
        }
        RemoteRuntimePhase::Stopped { .. } => {
            return Err(RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable).into());
        }
        _ => {}
    }
    let panel = state
        .spec
        .panels
        .iter()
        .find(|panel| panel.panel_local_id == request.panel_id)
        .ok_or(RemotePanelAttachError::from(RemotePanelStatusError::UnknownPanel))?;
    if !matches!(panel.kind, PanelKind::Shell | PanelKind::Command)
        || panel.command.is_none()
        || panel.task_handoff.is_some()
        || panel.agent_session_id.is_some()
    {
        return Err(RemotePanelAttachError::from(RemotePanelStatusError::UnsupportedIntent).into());
    }
    let expected = admitted
        .allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    identities
        .recover(expected.workflow_id, expected.job_id, &expected.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    // The client (and its lazy credential) is built first, then the saved state is
    // rechecked, so drift during that step is a changed state even when the client
    // could not be built.
    let provider = client(&admitted);
    admitted.check_current(store, &admitted.allocation)?;
    let bound = Bound::new(provider?, store, &admitted);
    let result = attach(
        &bound,
        RemotePanelAttachRequest {
            allocation: &admitted.allocation,
            panel_id: request.panel_id,
            terminal: request.terminal,
        },
    );
    if bound.drifted() {
        return Err(RemotePanelAttachError::StateChanged.into());
    }
    result.map_err(Into::into)
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredRemotePanelAttachError {
    #[error("panel reconnection is not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
    #[error("RunPod reconnection requires a valid RUNPOD_API_KEY supplied to the controller")]
    RunPodCredentialUnavailable,
    #[error("the configured RunPod profile or retained worker and storage binding is invalid")]
    InvalidRunPodBinding,
    #[error(
        "the configured Azure profile or retained worker, public pin, profile binding and storage binding is invalid"
    )]
    InvalidAzureBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Attachment(#[from] RemotePanelAttachError),
}

impl From<RemoteWorkspaceRecoveryError> for ConfiguredRemotePanelAttachError {
    fn from(error: RemoteWorkspaceRecoveryError) -> Self {
        Self::Attachment(error.into())
    }
}

impl From<BindingError> for ConfiguredRemotePanelAttachError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::UnsupportedProvider => Self::UnsupportedProvider,
            BindingError::CredentialUnavailable => Self::RunPodCredentialUnavailable,
            BindingError::InvalidBinding => Self::InvalidAzureBinding,
            BindingError::Configuration(error) => Self::Configuration(error),
            BindingError::Stop(error) => match error {
                RemoteWorkspaceStopError::MissingAllocation => RemoteWorkspaceRecoveryError::MissingAllocation.into(),
                RemoteWorkspaceStopError::ProviderMismatch => RemoteWorkspaceRecoveryError::ProviderMismatch.into(),
                RemoteWorkspaceStopError::StorageUnavailable => RemoteWorkspaceRecoveryError::StorageUnavailable.into(),
                RemoteWorkspaceStopError::StateChanged => RemotePanelAttachError::StateChanged.into(),
                RemoteWorkspaceStopError::UnsupportedLifetime => RemotePanelAttachError::UnsupportedLifetime.into(),
                RemoteWorkspaceStopError::ManagementConflict => {
                    RemotePanelAttachError::from(RemotePanelStatusError::ManagementPending).into()
                }
                RemoteWorkspaceStopError::MissingWorker
                | RemoteWorkspaceStopError::MissingTrust
                | RemoteWorkspaceStopError::ProviderUnavailable
                | RemoteWorkspaceStopError::ResourceAbsent => {
                    RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable).into()
                }
                // Stop-only outcomes; the attachment admission never produces them.
                RemoteWorkspaceStopError::MissingStopIntent | RemoteWorkspaceStopError::InvalidTimestamp => {
                    Self::InvalidAzureBinding
                }
            },
        }
    }
}
