//! Confirmation binds saved intent, not a summary or an inert view catalog.

use super::{
    CloudWorkflowStore, RemoteGitTaskStartError, RemotePanelStatus, RemoteSshIdentityStore, StoredRemoteAllocation,
};
use crate::{
    cloud_run::runpod::RunPodNetworkVolumeExpectation,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_worker_status::ConfiguredRemotePanelStatusRequest,
    remote_workspace::RemoteEnvironmentSummary,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Opaque, single-use confirmation snapshot. Display only these saved values;
/// do not interpret the preview as live repository readiness or task authority.
#[derive(Eq, PartialEq)]
pub struct PreparedRemoteGitStart {
    allocation: StoredRemoteAllocation,
    selection: Option<RunPodNetworkVolumeExpectation>,
    config: RemoteProviderConfig,
    expected: RemoteEnvironmentSummary,
    panel: String,
    directory: String,
    argv: Vec<String>,
    branch: String,
}

impl PreparedRemoteGitStart {
    #[must_use]
    pub fn repository(&self) -> &str {
        &self.allocation.workspace().state().spec.repository.repository
    }
    #[must_use]
    pub fn commit(&self) -> &str {
        self.allocation.workspace().state().spec.repository.commit.as_str()
    }
    #[must_use]
    pub fn work_branch(&self) -> &str {
        &self.branch
    }
    #[must_use]
    pub fn working_directory(&self) -> &str {
        &self.directory
    }
    #[must_use]
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    #[must_use]
    pub fn panel_id(&self) -> &str {
        &self.panel
    }
}

/// Prepare off-thread using only existing local state and non-secret configuration.
/// No key reads, provider requests, repair, allocation or task execution occurs.
/// # Errors
/// Rejects foreign/stale selections, unsupported saved intents, nonpersistent
/// workers, missing public trust and invalid explicit provider configuration.
pub fn prepare_configured_remote_git_start(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
) -> Result<PreparedRemoteGitStart, ConfiguredRemoteGitStartError> {
    #[cfg(target_os = "linux")]
    {
        prepare(store, config, request)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, request);
        Err(RemoteGitTaskStartError::UnsupportedPlatform.into())
    }
}

/// Consume the displayed confirmation, rechecking its complete local binding.
/// Runs off-thread. The caller must pass the actual current session/selection and
/// configuration, and report changes after dispatch as unknown, never retry them.
/// Only the existing bounded, pinned, one-shot saved-Shell start path is invoked.
/// # Errors
/// Pre-dispatch drift is rejected; drift after possible execution is unknown.
pub fn start_configured_remote_git_shell(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
    prepared: PreparedRemoteGitStart,
) -> Result<RemotePanelStatus, ConfiguredRemoteGitStartError> {
    #[cfg(target_os = "linux")]
    {
        let result = confirmed(store, config, request, &prepared, || {
            dispatch(store, identities, &prepared)
        });
        drop(prepared);
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request, prepared);
        Err(RemoteGitTaskStartError::UnsupportedPlatform.into())
    }
}

#[cfg(target_os = "linux")]
fn prepare(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
) -> Result<PreparedRemoteGitStart, ConfiguredRemoteGitStartError> {
    use crate::cloud_run::{CloudProvider, WorkerLifetime, runpod::validate_target};
    use ConfiguredRemoteGitStartError::{InvalidBinding, StateChanged};
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemoteGitStartError::ClientSessionMismatch);
    }
    let allocation = store
        .load_remote_allocation(request.client_session_id, &request.expected.workspace_local_id)
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if allocation.workspace().environment_summary() != *request.expected {
        return Err(StateChanged);
    }
    let saved = allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if saved.target.lifetime != WorkerLifetime::Persistent {
        return Err(InvalidBinding);
    }
    super::request(&allocation, request.panel_id)?;
    let state = allocation.workspace().state();
    let (worker, ssh) = state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref().zip(runtime.ssh.as_ref()))
        .ok_or(RemoteGitTaskStartError::MissingRetainedWorker)?;
    if !ssh.is_complete()
        || !worker.is_valid_for(saved.target.provider)
        || worker.identity.workflow_id != saved.workflow_id
        || worker.identity.job_id != saved.job_id
        || worker.target != saved.target
        || worker.ssh_public_key != saved.ssh_public_key
    {
        return Err(InvalidBinding);
    }
    let selection = store
        .load_remote_network_volume_selection(&allocation)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    match saved.target.provider {
        CloudProvider::LocalDocker => {
            config.local_docker_profile(&saved.target.profile)?;
        }
        CloudProvider::RunPod => {
            let profile = config.runpod_profile(&saved.target.profile)?;
            validate_target(&saved.target, profile).map_err(|_| InvalidBinding)?;
            crate::cloud_run::runpod::RunPodHostTrust::retained(worker, ssh).map_err(|_| InvalidBinding)?;
            if selection.as_ref().is_some_and(|volume| {
                profile
                    .data_center_id
                    .as_ref()
                    .is_some_and(|id| id != &volume.data_center_id)
            }) {
                return Err(InvalidBinding);
            }
        }
        CloudProvider::Azure => return Err(ConfiguredRemoteGitStartError::UnsupportedProvider),
    }
    let panel = state
        .spec
        .panels
        .iter()
        .find(|panel| panel.panel_local_id == request.panel_id)
        .ok_or(RemoteGitTaskStartError::InvalidIntent)?;
    let command = panel.command.as_ref().ok_or(RemoteGitTaskStartError::InvalidIntent)?;
    let directory = panel
        .working_directory
        .as_ref()
        .unwrap_or(&state.spec.working_directory)
        .clone();
    let argv = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect();
    let branch = state
        .spec
        .repository
        .branch
        .clone()
        .ok_or(RemoteGitTaskStartError::InvalidIntent)?;
    Ok(PreparedRemoteGitStart {
        allocation,
        selection,
        config: config.clone(),
        expected: request.expected.clone(),
        panel: request.panel_id.into(),
        directory,
        argv,
        branch,
    })
}

#[cfg(target_os = "linux")]
fn confirmed(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemotePanelStatusRequest<'_>,
    prepared: &PreparedRemoteGitStart,
    execute: impl FnOnce() -> Result<RemotePanelStatus, ConfiguredRemoteGitStartError>,
) -> Result<RemotePanelStatus, ConfiguredRemoteGitStartError> {
    let check = || {
        let current = prepare(store, config, request)?;
        if current != *prepared {
            return Err(ConfiguredRemoteGitStartError::StateChanged);
        }
        Ok(())
    };
    check()?;
    let result = execute();
    // Never use `?` on the exchange before checking post-dispatch drift.
    check().map_err(|_| ConfiguredRemoteGitStartError::OutcomeUnknown)?;
    result
}

#[cfg(target_os = "linux")]
fn dispatch(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    prepared: &PreparedRemoteGitStart,
) -> Result<RemotePanelStatus, ConfiguredRemoteGitStartError> {
    use crate::cloud_run::{
        CloudProvider,
        local_docker::LocalDockerInteractiveWorkerProvider,
        runpod::{RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider},
    };
    use ConfiguredRemoteGitStartError::InvalidBinding;
    let allocation = &prepared.allocation;
    let saved = allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    identities
        .recover(saved.workflow_id, saved.job_id, &saved.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if saved.target.provider == CloudProvider::LocalDocker {
        let profile = prepared.config.local_docker_profile(&saved.target.profile)?;
        let provider =
            LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone()).map_err(|_| InvalidBinding)?;
        return super::start_remote_git_shell(store, identities, &provider, allocation, &prepared.panel)
            .map_err(Into::into);
    }
    let profile = prepared.config.runpod_profile(&saved.target.profile)?;
    let (worker, ssh) = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref().zip(runtime.ssh.as_ref()))
        .ok_or(InvalidBinding)?;
    let trust = RunPodHostTrust::retained(worker, ssh).map_err(|_| InvalidBinding)?;
    let key = RunPodApiKey::from_env().map_err(|_| ConfiguredRemoteGitStartError::CredentialUnavailable)?;
    let client = RunPodClient::new(&key, store.clone());
    let provider = match &prepared.selection {
        Some(volume) => {
            RunPodInteractiveWorkerProvider::new_with_network_volume(client, profile.clone(), trust, &saved, volume)
                .map_err(|_| InvalidBinding)?
        }
        None => RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust),
    };
    super::start_remote_git_shell(store, identities, &provider, allocation, &prepared.panel).map_err(Into::into)
}

/// Static diagnostics never include saved argv, private paths or provider output.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ConfiguredRemoteGitStartError {
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
    #[error("confirmed task start supports only configured Local Docker and RunPod workers")]
    UnsupportedProvider,
    #[error("task start requires a persistent, valid configured worker and retained binding")]
    InvalidBinding,
    #[error("the confirmed task or environment changed; prepare a new confirmation")]
    StateChanged,
    #[error("task start outcome is unknown; inspect without automatic retry")]
    OutcomeUnknown,
    #[error("RunPod task start requires RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Start(RemoteGitTaskStartError),
}

impl From<RemoteGitTaskStartError> for ConfiguredRemoteGitStartError {
    fn from(error: RemoteGitTaskStartError) -> Self {
        if error == RemoteGitTaskStartError::OutcomeUnknown {
            Self::OutcomeUnknown
        } else {
            Self::Start(error)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
