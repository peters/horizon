//! Explicit selected-panel observation without recovery persistence or attachment.

use super::{RemotePanelStatus, RemotePanelStatusError};
use crate::{
    cloud_run::CloudWorkflowStore,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::RemoteEnvironmentSummary,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

#[derive(Clone, Copy)]
pub struct ConfiguredRemotePanelStatusRequest<'a> {
    pub expected: &'a RemoteEnvironmentSummary,
    /// Actual resolved persistent client session, never copied from an inventory claim.
    pub client_session_id: &'a str,
    pub panel_id: &'a str,
}

/// Overview-safe point-in-time status, not task intent, repository or connection readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemotePanelObservation {
    pub panel_id: String,
    pub status: RemotePanelStatus,
    pub observed_at_millis: i64,
}

impl RemotePanelObservation {
    /// Absolute UTC label; no ticking clock or continued freshness is implied.
    #[must_use]
    pub fn observed_at_rfc3339(&self) -> Option<String> {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.observed_at_millis) * 1_000_000)
            .ok()?
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
    }
}

/// Check one owned saved task through its explicit local provider and retained SSH pin.
/// All storage, retained-key, provider and SSH work must run off the render thread.
/// Requires an already recorded worker and host pin; never repairs missing identity.
/// No key creation, recovery persistence, task startup, local view, attachment,
/// allocation, Stop, Delete or provider fallback occurs. The caller must discard
/// results after client/session, selection or configuration changes. Observations
/// are not continuous monitoring or authority for subsequent lifecycle actions.
/// # Errors
/// Rejects unsupported providers/platforms, foreign or stale selections, unknown
/// panels, pending management, missing retained identity and failed bounded queries.
pub fn inspect_configured_remote_panel(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
) -> Result<RemotePanelObservation, ConfiguredRemotePanelStatusError> {
    #[cfg(target_os = "linux")]
    {
        inspect_with(
            store,
            identities,
            config,
            request,
            |profile| {
                crate::cloud_run::local_docker::LocalDockerInteractiveWorkerProvider::new(
                    profile.clone(),
                    store.clone(),
                )
                .map_err(|_| RemoteWorkspaceRecoveryError::ProviderUnavailable.into())
            },
            super::inspect_remote_panel,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request);
        Err(RemotePanelStatusError::UnsupportedPlatform.into())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn inspect_with<P: crate::cloud_run::interactive_worker::InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
    provider: impl FnOnce(
        &crate::cloud_run::local_docker::LocalDockerProfile,
    ) -> Result<P, ConfiguredRemotePanelStatusError>,
    inspect: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &str,
    ) -> Result<RemotePanelStatus, RemotePanelStatusError>,
) -> Result<RemotePanelObservation, ConfiguredRemotePanelStatusError> {
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemotePanelStatusError::ClientSessionMismatch);
    }
    if request.expected.provider != crate::cloud_run::CloudProvider::LocalDocker {
        return Err(ConfiguredRemotePanelStatusError::UnsupportedProvider);
    }
    let profile = config.local_docker_profile(&request.expected.profile)?;
    let allocation = store
        .load_remote_allocation(request.client_session_id, &request.expected.workspace_local_id)
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if allocation.workspace().environment_summary() != *request.expected {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let state = allocation.workspace().state();
    if !state
        .spec
        .panels
        .iter()
        .any(|panel| panel.panel_local_id == request.panel_id)
    {
        return Err(RemotePanelStatusError::UnknownPanel.into());
    }
    if !state
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.worker.is_some() && runtime.ssh.is_some())
    {
        return Err(RemotePanelStatusError::WorkerUnavailable.into());
    }
    let provider = provider(profile)?;
    let recovered = crate::remote_workspace_recovery::inspect_remote_allocation(identities, &provider, &allocation)?;
    let status = inspect(store, &recovered, request.panel_id)?;
    let observed_at_millis = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| RemotePanelStatusError::InvalidResponse)?;
    Ok(RemotePanelObservation {
        panel_id: request.panel_id.into(),
        status,
        observed_at_millis,
    })
}

/// Diagnostics contain no provider output, task payload, SSH coordinates or private paths.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredRemotePanelStatusError {
    #[error("task checks are not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Inspection(#[from] RemotePanelStatusError),
}
