//! Consumed, home-bound authorization for one new task-free worker, not repository or task execution.

use crate::{
    HorizonHome, PanelKind,
    cloud_run::{
        CloudProvider, CloudWorkflowStore, GitSource, RemoteWorkspaceStoreError, StoredRemoteAllocation,
        StoredRemoteWorkspace, WorkerLifetime, WorkerTarget,
        local_docker::LocalDockerInteractiveWorkerProvider,
        runpod::{RunPodApiKey, RunPodHostTrust, RunPodNetworkVolumeExpectation, validate_target},
    },
    remote_provider_config::RemoteProviderConfig,
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::{RemotePanelBinding, RemotePanelCommand, RemoteWorkspaceSpec, RemoteWorkspaceState},
};
use ConfiguredWorkspaceSetupError as Error;
use std::path::PathBuf;

/// Non-secret caller input. No defaults select an image, profile, branch, command or storage.
pub struct RemoteWorkspaceSetupDraft {
    pub target: WorkerTarget,
    pub repository: GitSource,
    pub working_directory: String,
    pub command: RemotePanelCommand,
    pub panel_directory: Option<String>,
    /// Absolute setup authorization expiry, never a persistent worker execution deadline.
    pub retain_until_millis: i64,
    pub network_volume: Option<RunPodNetworkVolumeExpectation>,
}

/// Recovery coordinates, not authority. Preserve before dispatch and after every failure.
#[derive(Clone, Eq, PartialEq)]
pub struct RemoteWorkspaceSetupLocator {
    home: PathBuf,
    pub owning_session_id: String,
    pub workspace_local_id: String,
}

impl RemoteWorkspaceSetupLocator {
    /// Reconstruct coordinates from saved inventory in the selected home, without I/O.
    /// # Errors
    /// Rejects noncanonical owner IDs and malformed workspace identities.
    pub fn new(home: &HorizonHome, owner: &str, workspace: &str) -> Result<Self, Error> {
        if !uuid::Uuid::parse_str(owner).is_ok_and(|id| !id.is_nil() && id.to_string() == owner)
            || !crate::remote_workspace::valid_local_id(workspace)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(Self {
            home: std::path::absolute(home.root()).map_err(|_| Error::InvalidRequest)?,
            owning_session_id: owner.into(),
            workspace_local_id: workspace.into(),
        })
    }

    fn check(&self, home: &HorizonHome, owner: &str) -> Result<(), Error> {
        if Self::new(home, owner, &self.workspace_local_id)? != *self {
            return Err(Error::ContextChanged);
        }
        Ok(())
    }
}

/// No Clone or deserialization: submission consumes the displayed, immutable confirmation.
pub struct PreparedRemoteWorkspaceSetup {
    locator: RemoteWorkspaceSetupLocator,
    config: RemoteProviderConfig,
    state: RemoteWorkspaceState,
    retain_until_millis: i64,
    network_volume: Option<RunPodNetworkVolumeExpectation>,
}

impl PreparedRemoteWorkspaceSetup {
    #[must_use]
    pub fn locator(&self) -> &RemoteWorkspaceSetupLocator {
        &self.locator
    }
    #[must_use]
    pub fn spec(&self) -> &RemoteWorkspaceSpec {
        &self.state.spec
    }
    #[must_use]
    pub fn network_volume(&self) -> Option<&RunPodNetworkVolumeExpectation> {
        self.network_volume.as_ref()
    }
    #[must_use]
    pub fn retain_until_millis(&self) -> i64 {
        self.retain_until_millis
    }
}

/// Positive authorization for the exact displayed image and, for `RunPod`, volume.
/// The caller independently trusts the entrypoint-only image and supplied storage
/// contents, authorizes their use and persistent billing, and permits no tasks before
/// first pin. This is not ownership, exclusivity, image or filesystem attestation.
pub enum RemoteWorkspaceSetupConsent {
    LocalDocker {
        image: String,
    },
    RunPodHps {
        image: String,
        volume: RunPodNetworkVolumeExpectation,
    },
}

/// Even an error retains the original locator. Errors after saving do not prove absence.
pub struct ConfiguredWorkspaceSetupAttempt {
    pub locator: RemoteWorkspaceSetupLocator,
    pub result: Result<StoredRemoteAllocation, Error>,
}

/// Point-in-time saved setup state, never repository/task readiness or permission to replay.
pub enum ConfiguredWorkspaceSetupObservation {
    Missing,
    SavedOnly(StoredRemoteWorkspace),
    Interrupted(StoredRemoteWorkspace),
    /// Noncreating recovery completed; even this may retain an absent/unavailable worker.
    Observed(StoredRemoteAllocation),
}

/// Validate only local values; no store, key, environment or provider access occurs.
/// IDs are generated once for this confirmation. Caller supplies the actual persistent
/// session, not an inventory owner or copied view. Run off the render thread.
/// # Errors
/// Rejects unsupported platforms/providers, invalid intent, profile, storage and expiry.
pub fn preview_configured_remote_workspace(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    client_session_id: &str,
    draft: RemoteWorkspaceSetupDraft,
) -> Result<PreparedRemoteWorkspaceSetup, Error> {
    platform()?;
    let locator = RemoteWorkspaceSetupLocator::new(home, client_session_id, &uuid::Uuid::new_v4().to_string())?;
    let state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: locator.workspace_local_id.clone(),
        target: draft.target,
        repository: draft.repository,
        working_directory: draft.working_directory,
        generation: 0,
        panels: vec![RemotePanelBinding {
            panel_local_id: uuid::Uuid::new_v4().to_string(),
            kind: PanelKind::Shell,
            command: Some(draft.command),
            working_directory: draft.panel_directory,
            task_handoff: None,
            agent_session_id: None,
        }],
    })
    .map_err(|_| Error::InvalidRequest)?;
    if state.spec.repository.branch.is_none() {
        return Err(Error::InvalidRequest);
    }
    validate_selection(config, &state.spec.target, draft.network_volume.as_ref())?;
    valid_expiry(draft.retain_until_millis)?;
    Ok(PreparedRemoteWorkspaceSetup {
        locator,
        config: config.clone(),
        state,
        retain_until_millis: draft.retain_until_millis,
        network_volume: draft.network_volume,
    })
}

/// Consume explicit consent, save once, and call the existing task-free setup operation.
/// Validation precedes writable storage and credential lookup. A duplicate never adopts
/// an existing record. No PAT, Git, task, attachment or compensating cleanup is dispatched.
/// Closing the client cannot revoke an admitted operation. Run off the render thread.
#[must_use]
pub fn submit_configured_remote_workspace(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    client_session_id: &str,
    prepared: PreparedRemoteWorkspaceSetup,
    consent: RemoteWorkspaceSetupConsent,
) -> ConfiguredWorkspaceSetupAttempt {
    let result = submit_with(home, config, client_session_id, &prepared, &consent, |store, saved| {
        let identities = RemoteSshIdentityStore::new(home);
        match saved.state().spec.target.provider {
            CloudProvider::LocalDocker => super::start_remote_workspace(
                store,
                &identities,
                &local_provider(store, config, &saved.state().spec.target)?,
                saved,
                prepared.retain_until_millis,
            )
            .map_err(|_| Error::SetupUnconfirmed),
            CloudProvider::RunPod => super::start_task_free_runpod_workspace_with_network_volume(
                store,
                &identities,
                &credential()?,
                config
                    .runpod_profile(&saved.state().spec.target.profile)
                    .map_err(|_| Error::InvalidProfile)?,
                saved,
                prepared.retain_until_millis,
                prepared.network_volume.as_ref().ok_or(Error::ConsentMismatch)?,
            )
            .map_err(|_| Error::SetupUnconfirmed),
            CloudProvider::Azure => Err(Error::UnsupportedProvider),
        }
    });
    drop(consent);
    ConfiguredWorkspaceSetupAttempt {
        locator: prepared.locator,
        result,
    }
}

fn submit_with(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    owner: &str,
    prepared: &PreparedRemoteWorkspaceSetup,
    consent: &RemoteWorkspaceSetupConsent,
    start: impl FnOnce(&CloudWorkflowStore, &StoredRemoteWorkspace) -> Result<StoredRemoteAllocation, Error>,
) -> Result<StoredRemoteAllocation, Error> {
    platform()?;
    prepared.locator.check(home, owner)?;
    if *config != prepared.config {
        return Err(Error::ContextChanged);
    }
    let target = &prepared.state.spec.target;
    let authorized = match consent {
        RemoteWorkspaceSetupConsent::LocalDocker { image } => {
            target.provider == CloudProvider::LocalDocker && *image == target.image && prepared.network_volume.is_none()
        }
        RemoteWorkspaceSetupConsent::RunPodHps { image, volume } => {
            target.provider == CloudProvider::RunPod
                && *image == target.image
                && prepared.network_volume.as_ref() == Some(volume)
        }
    };
    if !authorized {
        return Err(Error::ConsentMismatch);
    }
    validate_selection(config, target, prepared.network_volume.as_ref())?;
    valid_expiry(prepared.retain_until_millis)?;
    let store = CloudWorkflowStore::open(home).map_err(|_| Error::StorageUnavailable)?;
    let saved = store
        .create_remote_workspace(owner, &prepared.state)
        .map_err(|error| match error {
            RemoteWorkspaceStoreError::AlreadyExists => Error::SaveConflict,
            _ => Error::StorageUnavailable,
        })?;
    let allocation = start(&store, &saved)?;
    let mut spec = allocation.workspace().state().spec.clone();
    if spec.generation != 1 {
        return Err(Error::SetupUnconfirmed);
    }
    spec.generation = 0;
    if spec != prepared.state.spec || allocation.workspace().session_id() != owner {
        return Err(Error::SetupUnconfirmed);
    }
    check_result(&store, &allocation, prepared.network_volume.as_ref())?;
    Ok(allocation)
}

/// Manually recover only the original saved setup. No key/intent repair, allocation,
/// ensure, creation, renewal or cleanup is allowed. A dormant or interrupted pre-intent
/// setup is reported locally. Eligible recovery may persist a pin/observation.
/// Current profile validation cannot attest historical edits under the same name.
/// Run off-thread; caller discards results on current session/config/selection drift.
/// # Errors
/// Rejects foreign homes/owners, invalid bindings, missing storage and recovery failures.
pub fn check_configured_remote_workspace_setup(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    client_session_id: &str,
    locator: &RemoteWorkspaceSetupLocator,
) -> Result<ConfiguredWorkspaceSetupObservation, Error> {
    check_with(home, config, client_session_id, locator, |store, allocation| {
        let identities = RemoteSshIdentityStore::new(home);
        let target = &allocation.workspace().state().spec.target;
        match target.provider {
            CloudProvider::LocalDocker => {
                super::recover(store, &identities, &local_provider(store, config, target)?, allocation)
                    .map_err(|_| Error::SetupUnconfirmed)
            }
            CloudProvider::RunPod => super::recover_runpod_workspace(
                store,
                &identities,
                &credential()?,
                config
                    .runpod_profile(&target.profile)
                    .map_err(|_| Error::InvalidProfile)?,
                allocation,
            )
            .map_err(|_| Error::SetupUnconfirmed),
            CloudProvider::Azure => Err(Error::UnsupportedProvider),
        }
    })
}

fn check_with(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    owner: &str,
    locator: &RemoteWorkspaceSetupLocator,
    recover: impl FnOnce(&CloudWorkflowStore, &StoredRemoteAllocation) -> Result<StoredRemoteAllocation, Error>,
) -> Result<ConfiguredWorkspaceSetupObservation, Error> {
    platform()?;
    locator.check(home, owner)?;
    let store = CloudWorkflowStore::open_read_only(home).map_err(|_| Error::StorageUnavailable)?;
    let Some(saved) = store
        .load_remote_workspace(owner, &locator.workspace_local_id)
        .map_err(storage)?
    else {
        return Ok(ConfiguredWorkspaceSetupObservation::Missing);
    };
    check_saved_with(home, config, &store, saved, recover)
}

fn check_saved_with(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    store: &CloudWorkflowStore,
    saved: StoredRemoteWorkspace,
    recover: impl FnOnce(&CloudWorkflowStore, &StoredRemoteAllocation) -> Result<StoredRemoteAllocation, Error>,
) -> Result<ConfiguredWorkspaceSetupObservation, Error> {
    validate_profile(config, &saved.state().spec.target)?;
    if saved.state().runtime.is_none() {
        return Ok(ConfiguredWorkspaceSetupObservation::SavedOnly(saved));
    }
    let allocation = store
        .load_remote_allocation(saved.session_id(), &saved.state().spec.workspace_local_id)
        .map_err(storage)?
        .ok_or(Error::SetupUnconfirmed)?;
    if allocation.workspace() != &saved {
        return Err(Error::ContextChanged);
    }
    let selection = store
        .load_remote_network_volume_selection(&allocation)
        .map_err(storage)?;
    match allocation.recovery_request() {
        Ok(_) => {}
        Err(RemoteWorkspaceStoreError::RuntimeRequestRequired) => {
            return Ok(ConfiguredWorkspaceSetupObservation::Interrupted(saved));
        }
        Err(_) => return Err(Error::SetupUnconfirmed),
    }
    if saved.state().spec.target.provider == CloudProvider::RunPod {
        if selection.is_none() {
            return Ok(ConfiguredWorkspaceSetupObservation::Interrupted(saved));
        }
        let runtime = saved.state().runtime.as_ref().ok_or(Error::SetupUnconfirmed)?;
        if let Some(ssh) = &runtime.ssh {
            let worker = runtime.worker.as_ref().ok_or(Error::InvalidRequest)?;
            RunPodHostTrust::retained(worker, ssh).map_err(|_| Error::InvalidRequest)?;
        } else if store
            .load_remote_first_pin_request(&allocation)
            .map_err(storage)?
            .is_none()
        {
            return Ok(ConfiguredWorkspaceSetupObservation::Interrupted(saved));
        }
    }
    validate_selection(config, &saved.state().spec.target, selection.as_ref())?;
    // Only an existing, validated eligible allocation reaches the writable recovery store.
    let writable = CloudWorkflowStore::open(home).map_err(|_| Error::StorageUnavailable)?;
    super::validate_allocation(&writable, &allocation).map_err(|_| Error::ContextChanged)?;
    let result = recover(&writable, &allocation)?;
    if result.workspace().session_id() != saved.session_id() || result.workspace().state().spec != saved.state().spec {
        return Err(Error::SetupUnconfirmed);
    }
    check_result(&writable, &result, selection.as_ref())?;
    Ok(ConfiguredWorkspaceSetupObservation::Observed(result))
}

fn check_result(
    store: &CloudWorkflowStore,
    result: &StoredRemoteAllocation,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<(), Error> {
    super::validate_allocation(store, result).map_err(|_| Error::SetupUnconfirmed)?;
    if store
        .load_remote_network_volume_selection(result)
        .map_err(storage)?
        .as_ref()
        != selection
    {
        return Err(Error::SetupUnconfirmed);
    }
    Ok(())
}

fn validate_profile(config: &RemoteProviderConfig, target: &WorkerTarget) -> Result<(), Error> {
    if target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::InvalidRequest);
    }
    match target.provider {
        CloudProvider::LocalDocker => {
            config
                .local_docker_profile(&target.profile)
                .map_err(|_| Error::InvalidProfile)?;
        }
        CloudProvider::RunPod => {
            let profile = config
                .runpod_profile(&target.profile)
                .map_err(|_| Error::InvalidProfile)?;
            validate_target(target, profile).map_err(|_| Error::InvalidProfile)?;
            if target.max_hourly_cost_micros.is_none() {
                return Err(Error::InvalidRequest);
            }
        }
        CloudProvider::Azure => return Err(Error::UnsupportedProvider),
    }
    Ok(())
}

fn validate_selection(
    config: &RemoteProviderConfig,
    target: &WorkerTarget,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<(), Error> {
    validate_profile(config, target)?;
    match (target.provider, selection) {
        (CloudProvider::LocalDocker, None) => Ok(()),
        (CloudProvider::RunPod, Some(volume)) => {
            volume.validate().map_err(|_| Error::InvalidRequest)?;
            let profile = config
                .runpod_profile(&target.profile)
                .map_err(|_| Error::InvalidProfile)?;
            if profile
                .data_center_id
                .as_ref()
                .is_some_and(|id| *id != volume.data_center_id)
            {
                return Err(Error::InvalidRequest);
            }
            Ok(())
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn local_provider(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    target: &WorkerTarget,
) -> Result<LocalDockerInteractiveWorkerProvider, Error> {
    let profile = config
        .local_docker_profile(&target.profile)
        .map_err(|_| Error::InvalidProfile)?;
    LocalDockerInteractiveWorkerProvider::new(profile.clone(), store.clone()).map_err(|_| Error::InvalidProfile)
}

fn credential() -> Result<RunPodApiKey, Error> {
    RunPodApiKey::from_env().map_err(|_| Error::CredentialUnavailable)
}

fn valid_expiry(expiry: i64) -> Result<(), Error> {
    if i128::from(expiry) <= time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000 {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

fn platform() -> Result<(), Error> {
    if !cfg!(target_os = "linux") {
        return Err(Error::UnsupportedPlatform);
    }
    Ok(())
}

fn storage(_: RemoteWorkspaceStoreError) -> Error {
    Error::StorageUnavailable
}

/// Fixed diagnostics never echo task arguments, private paths or provider payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredWorkspaceSetupError {
    #[error("new remote workspace setup is supported only on Linux")]
    UnsupportedPlatform,
    #[error("this provider is not supported for new workspace setup")]
    UnsupportedProvider,
    #[error("remote workspace setup requires valid explicit intent, storage and expiry")]
    InvalidRequest,
    #[error("remote workspace setup requires an exact valid configured profile")]
    InvalidProfile,
    #[error("remote workspace home, owner, configuration or saved state changed")]
    ContextChanged,
    #[error("explicit consent does not match this worker image and storage selection")]
    ConsentMismatch,
    #[error("remote workspace identity is already saved; no existing record was adopted")]
    SaveConflict,
    #[error("remote control storage could not be verified; retain the original locator")]
    StorageUnavailable,
    #[error("RunPod credentials are unavailable; the saved workspace was retained")]
    CredentialUnavailable,
    #[error("worker setup is unconfirmed; retain the original locator and check without creating")]
    SetupUnconfirmed,
}

#[cfg(test)]
mod tests;
