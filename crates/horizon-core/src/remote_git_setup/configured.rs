//! Explicit saved-repository confirmation; credentials are never part of a preview.

#[cfg(target_os = "linux")]
mod azure;

use super::{RemoteGitObservation, RemoteGitSetupError, RemoteGitSubmission};
use crate::{
    cloud_run::{CloudWorkflowStore, StoredRemoteAllocation, runpod::RunPodNetworkVolumeExpectation},
    remote_github_credential::{RemoteCredentialDeliveryError, RemoteCredentialInstallation, RepositoryPat},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::RemoteEnvironmentSummary,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Caller supplies the actual current persistent client session and selection.
#[derive(Clone, Copy)]
pub struct ConfiguredRemoteGitSetupRequest<'a> {
    pub expected: &'a RemoteEnvironmentSummary,
    pub client_session_id: &'a str,
}

/// Installation is explicit first-token disclosure, not rotation or scope verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteGitCredentialMode {
    UseInstalled,
    InstallFirst,
}

/// Consumed confirmation of the complete saved binding; contains no repository PAT.
#[derive(Eq, PartialEq)]
pub struct PreparedRemoteGitSetup {
    allocation: StoredRemoteAllocation,
    selection: Option<RunPodNetworkVolumeExpectation>,
    config: RemoteProviderConfig,
    expected: RemoteEnvironmentSummary,
    branch: String,
    credentials: RemoteGitCredentialMode,
}

impl PreparedRemoteGitSetup {
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
    pub fn credential_mode(&self) -> RemoteGitCredentialMode {
        self.credentials
    }
    #[must_use]
    pub fn environment(&self) -> &RemoteEnvironmentSummary {
        &self.expected
    }
}

/// Point-in-time installer and handoff results, not GitHub scope or checkout readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfiguredRemoteGitSubmission {
    pub credential: Option<RemoteCredentialInstallation>,
    pub submission: RemoteGitSubmission,
}

/// Read existing local state and non-secret configuration off-thread. No provider,
/// private key, credential lookup or store mutation occurs while preparing a preview.
/// # Errors
/// Rejects foreign/stale selections, missing explicit branches, invalid profiles,
/// nonpersistent workers and incomplete retained trust.
pub fn prepare_configured_remote_git_setup(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    credentials: RemoteGitCredentialMode,
) -> Result<PreparedRemoteGitSetup, ConfiguredRemoteGitSetupError> {
    #[cfg(target_os = "linux")]
    {
        prepare(store, config, request, credentials)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, config, request, credentials);
        Err(RemoteGitSetupError::UnsupportedPlatform.into())
    }
}

/// Consume an explicitly displayed confirmation. A borrowed PAT is required only
/// for `InstallFirst`; the caller owns disclosure consent and its transient storage.
/// Install/refuse/unknown never silently rotates credentials or retries. Only a
/// confirmed install/present reply proceeds to the existing detached Git handoff.
/// Run off-thread; existing pinned transport time/lease limits remain unchanged.
/// # Errors
/// Rejects drift before dispatch; drift after possible mutation is `OutcomeUnknown`,
/// even when the exchange also failed. A failed Git phase may follow installation.
pub fn submit_configured_remote_git_setup(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    prepared: PreparedRemoteGitSetup,
    token: Option<&RepositoryPat<'_>>,
) -> Result<ConfiguredRemoteGitSubmission, ConfiguredRemoteGitSetupError> {
    if (prepared.credentials == RemoteGitCredentialMode::InstallFirst) != token.is_some() {
        return Err(ConfiguredRemoteGitSetupError::CredentialConsentMismatch);
    }
    #[cfg(target_os = "linux")]
    {
        check_snapshot(store, config, request, &prepared)?;
        let result = dispatch(store, identities, config, request, &prepared, token, false);
        drop(prepared);
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request, prepared, token);
        Err(RemoteGitSetupError::UnsupportedPlatform.into())
    }
}

/// Inspect the original receipt, without PAT installation, preparation or replay.
/// Complete with no reason is preparation evidence, not current task readiness.
/// # Errors
/// Refuses stale/foreign ownership, missing trust and unconfirmed observations.
pub fn inspect_configured_remote_git_setup(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
) -> Result<RemoteGitObservation, ConfiguredRemoteGitSetupError> {
    #[cfg(target_os = "linux")]
    {
        let prepared = prepare(store, config, request, RemoteGitCredentialMode::UseInstalled)?;
        match dispatch(store, identities, config, request, &prepared, None, true)?.submission {
            RemoteGitSubmission::Observed(observation) => Ok(observation),
            _ => Err(ConfiguredRemoteGitSetupError::OutcomeUnknown),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, config, request);
        Err(RemoteGitSetupError::UnsupportedPlatform.into())
    }
}

#[cfg(target_os = "linux")]
fn prepare(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    credentials: RemoteGitCredentialMode,
) -> Result<PreparedRemoteGitSetup, ConfiguredRemoteGitSetupError> {
    use crate::cloud_run::{CloudProvider, WorkerLifetime, runpod::validate_target};
    use ConfiguredRemoteGitSetupError::{InvalidBinding, StateChanged};
    if request.client_session_id != request.expected.owning_session_id {
        return Err(ConfiguredRemoteGitSetupError::ClientSessionMismatch);
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
    let state = allocation.workspace().state();
    let branch = state.spec.repository.branch.clone().ok_or(InvalidBinding)?;
    super::protocol::request(&allocation, &branch)?;
    let (worker, ssh) = state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref().zip(runtime.ssh.as_ref()))
        .ok_or(InvalidBinding)?;
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
        CloudProvider::Azure => {
            if selection.is_some() {
                return Err(InvalidBinding);
            }
            azure::profile(store, config, &allocation)?;
        }
    }
    Ok(PreparedRemoteGitSetup {
        allocation,
        selection,
        config: config.clone(),
        expected: request.expected.clone(),
        branch,
        credentials,
    })
}

#[cfg(target_os = "linux")]
fn check_snapshot(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    prepared: &PreparedRemoteGitSetup,
) -> Result<(), ConfiguredRemoteGitSetupError> {
    if prepare(store, config, request, prepared.credentials)? != *prepared {
        return Err(ConfiguredRemoteGitSetupError::StateChanged);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn submit_with(
    check: impl Fn() -> Result<(), ConfiguredRemoteGitSetupError>,
    token: Option<&RepositoryPat<'_>>,
    install: impl FnOnce(&RepositoryPat<'_>) -> Result<RemoteCredentialInstallation, ConfiguredRemoteGitSetupError>,
    submit: impl FnOnce() -> Result<RemoteGitSubmission, ConfiguredRemoteGitSetupError>,
) -> Result<ConfiguredRemoteGitSubmission, ConfiguredRemoteGitSetupError> {
    let credential = if let Some(token) = token {
        check()?;
        let result = install(token);
        check().map_err(|_| ConfiguredRemoteGitSetupError::OutcomeUnknown)?;
        Some(result?)
    } else {
        None
    };
    if credential.is_some() {
        check().map_err(|_| ConfiguredRemoteGitSetupError::OutcomeUnknown)?;
    } else {
        check()?;
    }
    let result = submit();
    check().map_err(|_| ConfiguredRemoteGitSetupError::OutcomeUnknown)?;
    Ok(ConfiguredRemoteGitSubmission {
        credential,
        submission: result?,
    })
}

#[cfg(target_os = "linux")]
fn perform<P: crate::cloud_run::interactive_worker::InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    check: impl Fn() -> Result<(), ConfiguredRemoteGitSetupError>,
    prepared: &PreparedRemoteGitSetup,
    token: Option<&RepositoryPat<'_>>,
    inspect: bool,
) -> Result<ConfiguredRemoteGitSubmission, ConfiguredRemoteGitSetupError> {
    if inspect {
        check()?;
        let result =
            super::inspect_remote_git_setup(store, identities, provider, &prepared.allocation, &prepared.branch);
        check()?;
        return Ok(ConfiguredRemoteGitSubmission {
            credential: None,
            submission: RemoteGitSubmission::Observed(result?),
        });
    }
    submit_with(
        check,
        token,
        |token| {
            crate::remote_github_credential::install_remote_github_credential(
                store,
                identities,
                provider,
                &prepared.allocation,
                token,
            )
            .map_err(Into::into)
        },
        || {
            super::submit_remote_git_setup(store, identities, provider, &prepared.allocation, &prepared.branch)
                .map_err(Into::into)
        },
    )
}

#[cfg(target_os = "linux")]
fn dispatch(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    request: ConfiguredRemoteGitSetupRequest<'_>,
    prepared: &PreparedRemoteGitSetup,
    token: Option<&RepositoryPat<'_>>,
    inspect: bool,
) -> Result<ConfiguredRemoteGitSubmission, ConfiguredRemoteGitSetupError> {
    use crate::cloud_run::{
        CloudProvider,
        local_docker::LocalDockerInteractiveWorkerProvider,
        runpod::{RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider},
    };
    use ConfiguredRemoteGitSetupError::InvalidBinding;
    let saved = prepared
        .allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    check_snapshot(store, config, request, prepared)?;
    identities
        .recover(saved.workflow_id, saved.job_id, &saved.ssh_public_key)
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let check = || check_snapshot(store, config, request, prepared);
    if saved.target.provider == CloudProvider::Azure {
        let provider = azure::client(store, &prepared.config, &prepared.allocation)?;
        return perform(store, identities, &provider, check, prepared, token, inspect);
    }
    if saved.target.provider == CloudProvider::LocalDocker {
        let profile = prepared.config.local_docker_profile(&saved.target.profile)?;
        let provider =
            LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone()).map_err(|_| InvalidBinding)?;
        return perform(store, identities, &provider, check, prepared, token, inspect);
    }
    let profile = prepared.config.runpod_profile(&saved.target.profile)?;
    let (worker, ssh) = prepared
        .allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref().zip(runtime.ssh.as_ref()))
        .ok_or(InvalidBinding)?;
    let trust = RunPodHostTrust::retained(worker, ssh).map_err(|_| InvalidBinding)?;
    let key = RunPodApiKey::from_env().map_err(|_| ConfiguredRemoteGitSetupError::CredentialUnavailable)?;
    let client = RunPodClient::new(&key, store.clone());
    let provider = match &prepared.selection {
        Some(volume) => {
            RunPodInteractiveWorkerProvider::new_with_network_volume(client, profile.clone(), trust, &saved, volume)
                .map_err(|_| InvalidBinding)?
        }
        None => RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust),
    };
    perform(store, identities, &provider, check, prepared, token, inspect)
}

/// Static errors never include PATs, saved commands or remote output.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredRemoteGitSetupError {
    #[error("the active client session does not own the selected environment")]
    ClientSessionMismatch,
    #[error("repository preparation requires a supported configured worker")]
    UnsupportedProvider,
    #[error("repository preparation requires a persistent worker, explicit saved branch and retained trust")]
    InvalidBinding,
    #[error("credential input does not match the displayed disclosure choice")]
    CredentialConsentMismatch,
    #[error("the confirmed environment or configuration changed; prepare a new confirmation")]
    StateChanged,
    #[error("repository preparation outcome is unknown; inspect without automatic retry")]
    OutcomeUnknown,
    #[error("RunPod repository preparation requires RUNPOD_API_KEY supplied to the controller")]
    CredentialUnavailable,
    #[error(transparent)]
    Configuration(#[from] RemoteProviderConfigError),
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Credential(RemoteCredentialDeliveryError),
    #[error(transparent)]
    Git(RemoteGitSetupError),
}

impl From<RemoteGitSetupError> for ConfiguredRemoteGitSetupError {
    fn from(error: RemoteGitSetupError) -> Self {
        if error == RemoteGitSetupError::OutcomeUnknown {
            Self::OutcomeUnknown
        } else {
            Self::Git(error)
        }
    }
}

impl From<RemoteCredentialDeliveryError> for ConfiguredRemoteGitSetupError {
    fn from(error: RemoteCredentialDeliveryError) -> Self {
        if error == RemoteCredentialDeliveryError::DeliveryUnknown {
            Self::OutcomeUnknown
        } else {
            Self::Credential(error)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
