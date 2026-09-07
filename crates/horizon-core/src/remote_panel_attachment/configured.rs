//! Explicit current-client and exact-profile admission; no ambient provider fallback.

use super::{RemotePanelAttachError, RemotePanelAttachRequest, RemotePanelConnectionAttempt, RemotePanelTerminalSize};
use crate::{
    cloud_run::{CloudProvider, CloudWorkflowStore, local_docker::LocalDockerInteractiveWorkerProvider},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::RemoteEnvironmentSummary,
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

/// Explicitly connect one existing panel through its exact configured local profile.
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
    if request.expected.provider != CloudProvider::LocalDocker {
        return Err(ConfiguredRemotePanelAttachError::UnsupportedProvider);
    }
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemotePanelAttachError::ClientSessionMismatch);
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

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredRemotePanelAttachError {
    #[error("panel reconnection is not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
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
