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

/// Check one owned saved task through its explicit local or `RunPod` provider and retained SSH pin.
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
        if request.expected.provider == crate::cloud_run::CloudProvider::RunPod {
            return runpod_with(
                store,
                identities,
                config,
                request,
                || {
                    crate::cloud_run::runpod::RunPodApiKey::from_env()
                        .map_err(|_| ConfiguredRemotePanelStatusError::RunPodCredentialUnavailable)
                },
                |provider, allocation| {
                    let recovered =
                        crate::remote_workspace_recovery::inspect_remote_allocation(identities, provider, allocation)?;
                    Ok(super::inspect_remote_panel(store, &recovered, request.panel_id)?)
                },
            );
        }
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
    observation(request.panel_id, status)
}

#[cfg(target_os = "linux")]
fn observation(
    panel_id: &str,
    status: RemotePanelStatus,
) -> Result<RemotePanelObservation, ConfiguredRemotePanelStatusError> {
    let observed_at_millis = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| RemotePanelStatusError::InvalidResponse)?;
    Ok(RemotePanelObservation {
        panel_id: panel_id.into(),
        status,
        observed_at_millis,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn runpod_with(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
    credential: impl FnOnce() -> Result<crate::cloud_run::runpod::RunPodApiKey, ConfiguredRemotePanelStatusError>,
    inspect: impl FnOnce(
        &crate::cloud_run::runpod::RunPodInteractiveWorkerProvider,
        &crate::cloud_run::StoredRemoteAllocation,
    ) -> Result<RemotePanelStatus, ConfiguredRemotePanelStatusError>,
) -> Result<RemotePanelObservation, ConfiguredRemotePanelStatusError> {
    use crate::cloud_run::{
        CloudProvider, WorkerLifetime,
        runpod::{RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider, validate_target},
    };
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemotePanelStatusError::ClientSessionMismatch);
    }
    if request.expected.provider != CloudProvider::RunPod {
        return Err(ConfiguredRemotePanelStatusError::UnsupportedProvider);
    }
    let profile = config.runpod_profile(&request.expected.profile)?;
    let allocation = store
        .load_remote_allocation(request.client_session_id, &request.expected.workspace_local_id)
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if allocation.workspace().environment_summary() != *request.expected {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    let expected = allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if expected.target.lifetime != WorkerLifetime::Persistent {
        return Err(ConfiguredRemotePanelStatusError::InvalidRunPodBinding);
    }
    validate_target(&expected.target, profile).map_err(|_| ConfiguredRemotePanelStatusError::InvalidRunPodBinding)?;
    let state = allocation.workspace().state();
    if !state
        .spec
        .panels
        .iter()
        .any(|panel| panel.panel_local_id == request.panel_id)
    {
        return Err(RemotePanelStatusError::UnknownPanel.into());
    }
    let (worker, ssh) = state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref().zip(runtime.ssh.as_ref()))
        .ok_or(RemotePanelStatusError::WorkerUnavailable)?;
    let trust =
        RunPodHostTrust::retained(worker, ssh).map_err(|_| ConfiguredRemotePanelStatusError::InvalidRunPodBinding)?;
    identities
        .recover(expected.workflow_id, expected.job_id, &expected.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let selection = store
        .load_remote_network_volume_selection(&allocation)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if selection.as_ref().is_some_and(|selection| {
        profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &selection.data_center_id)
    }) {
        return Err(ConfiguredRemotePanelStatusError::InvalidRunPodBinding);
    }
    let key = credential()?;
    let client = RunPodClient::new(&key, store.clone());
    let provider = match &selection {
        Some(selection) => RunPodInteractiveWorkerProvider::new_with_network_volume(
            client,
            profile.clone(),
            trust,
            &expected,
            selection,
        )
        .map_err(|_| ConfiguredRemotePanelStatusError::InvalidRunPodBinding)?,
        None => RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust),
    };
    let check_current = || {
        // This read transaction fences the complete allocation and separate immutable selection.
        if store
            .load_remote_network_volume_selection(&allocation)
            .map_err(RemoteWorkspaceRecoveryError::from)?
            != selection
        {
            return Err(ConfiguredRemotePanelStatusError::from(
                RemoteWorkspaceRecoveryError::StateChanged,
            ));
        }
        Ok(())
    };
    check_current()?;
    let status = inspect(&provider, &allocation)?;
    check_current()?;
    observation(request.panel_id, status)
}

/// Diagnostics contain no provider output, task payload, SSH coordinates or private paths.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredRemotePanelStatusError {
    #[error("task checks are not yet supported for this environment's provider")]
    UnsupportedProvider,
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
    #[error("RunPod task inspection requires a valid RUNPOD_API_KEY supplied to the controller")]
    RunPodCredentialUnavailable,
    #[error("the configured RunPod profile or retained worker and storage binding is invalid")]
    InvalidRunPodBinding,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Inspection(#[from] RemotePanelStatusError),
}
