//! Explicit supplied-profile setup; missing trust never authorizes first-pin bootstrap.

use super::{
    CloudWorkflowStore, InteractiveWorkerProvider, RemoteSshIdentityError, RemoteSshIdentityStore,
    RemoteWorkspaceRecoveryError, RemoteWorkspaceSetupError, RemoteWorkspaceStoreError, StoredRemoteAllocation,
    StoredRemoteWorkspace, prepare_identity, recover, retry_remote_workspace_setup, validate_allocation,
};
use crate::cloud_run::{
    WorkerLifetime, WorkerTarget,
    interactive_worker::InteractiveWorkerRequest,
    runpod::{
        RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider, RunPodNetworkVolumeExpectation,
        RunPodProfile, validate_target,
    },
};

/// Allocate one explicitly approved, task-free generation and retain first-pin intent.
/// The caller selects an approved digest-pinned entrypoint-only image and must permit
/// no arbitrary setup, task or attachment before the first pin is committed. Intent
/// is not creation or task authority. Run synchronously off the render thread.
/// A pre-intent failure preserves an unmarked allocation; retry must refuse it.
/// No config/credential lookup, implicit cleanup or readiness promotion occurs.
/// # Errors
/// Rejects unsupported platforms, invalid supplied profiles, stale selections and
/// identity/intent/provider failures. Admitted ensure retains its provider cleanup contract.
pub fn start_task_free_runpod_workspace(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteWorkspace,
    retain_until_millis: i64,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    start_with(
        store,
        identities,
        api_key,
        profile,
        expected,
        retain_until_millis,
        |trust, request, selection| provider(store, api_key, profile, trust, request, selection),
    )
}

/// Start one task-free persistent generation with a caller-authorized HPS selection.
/// Selection is saved before any client identity, first-pin intent or creation claim.
/// It proves neither volume ownership nor exclusivity, contents trust or durability.
/// Failures retain the allocation and any saved intent; retry/recovery never replace
/// a selection or repair an interrupted pre-intent start. No volume mutation occurs.
/// Run synchronously off the render thread.
/// # Errors
/// Rejects invalid profiles/selections, stale ownership and identity/intent/provider
/// failures. A selection-recording failure may leave an unmarked allocation.
pub fn start_task_free_runpod_workspace_with_network_volume(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteWorkspace,
    retain_until_millis: i64,
    selection: &RunPodNetworkVolumeExpectation,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    validate_profile(&expected.state().spec.target, profile)?;
    selection
        .validate()
        .map_err(|_| RunPodWorkspaceSetupError::InvalidNetworkVolumeSelection)?;
    if expected.state().spec.target.lifetime != WorkerLifetime::Persistent
        || profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &selection.data_center_id)
    {
        return Err(RunPodWorkspaceSetupError::NetworkVolumeBindingRejected);
    }
    let allocation = allocate_start(store, expected, retain_until_millis, Some(selection))?;
    finish_start(
        store,
        identities,
        api_key,
        profile,
        &allocation,
        |trust, request, selection| provider(store, api_key, profile, trust, request, selection),
    )
}

/// Retry this exact setup using positive initial intent or the complete retained pin.
/// Never prepares a replacement key or records new intent. Claimed, observed or
/// expired setup uses non-creating recovery. Success remains Reconciling, not ready.
/// Supplied profile validation does not detect historical edits under the same name.
/// Any saved volume selection is loaded unchanged; absence preserves ordinary setup.
/// Run synchronously off the render thread; the supplied store owns explicit writes.
/// # Errors
/// Rejects invalid profiles, missing trust/keys, management intent and snapshot drift.
pub fn retry_runpod_workspace_setup(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    dispatch(
        store,
        identities,
        api_key,
        profile,
        expected,
        Operation::Retry,
        |trust, request, selection| provider(store, api_key, profile, trust, request, selection),
    )
}

/// Inspect/reconcile only, even if the marked setup has never consumed a creation claim.
/// Trust selection and publication bind the same owned snapshot. Admission is
/// point-in-time: a later pin/management change can overlap inspection, but its stale
/// result cannot be committed. No continuous revocation or implicit cleanup is promised.
/// Run synchronously off the render thread. No credential/config discovery occurs.
/// # Errors
/// Rejects invalid profiles, missing trust/keys, management intent and snapshot drift.
pub fn recover_runpod_workspace(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    dispatch(
        store,
        identities,
        api_key,
        profile,
        expected,
        Operation::Recover,
        |trust, request, selection| provider(store, api_key, profile, trust, request, selection),
    )
}

#[derive(Clone, Copy)]
enum Operation {
    Retry,
    Recover,
}

enum TrustSelection {
    Initial(RunPodHostTrust),
    Retained(RunPodHostTrust),
}

fn provider(
    store: &CloudWorkflowStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    trust: TrustSelection,
    request: &InteractiveWorkerRequest,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<RunPodInteractiveWorkerProvider, RunPodWorkspaceSetupError> {
    let (TrustSelection::Initial(trust) | TrustSelection::Retained(trust)) = trust;
    let client = RunPodClient::new(api_key, store.clone());
    match selection {
        Some(selection) => {
            RunPodInteractiveWorkerProvider::new_with_network_volume(client, profile.clone(), trust, request, selection)
                .map_err(|_| RunPodWorkspaceSetupError::NetworkVolumeBindingRejected)
        }
        None => Ok(RunPodInteractiveWorkerProvider::new(client, profile.clone(), trust)),
    }
}

fn validate_profile(target: &WorkerTarget, profile: &RunPodProfile) -> Result<(), RunPodWorkspaceSetupError> {
    if !cfg!(target_os = "linux") {
        return Err(RemoteSshIdentityError::UnsupportedPlatform.into());
    }
    validate_target(target, profile).map_err(|_| RunPodWorkspaceSetupError::InvalidProfile)
}

fn start_with<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteWorkspace,
    retain_until_millis: i64,
    factory: impl FnOnce(
        TrustSelection,
        &InteractiveWorkerRequest,
        Option<&RunPodNetworkVolumeExpectation>,
    ) -> Result<P, RunPodWorkspaceSetupError>,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    validate_profile(&expected.state().spec.target, profile)?;
    let allocation = allocate_start(store, expected, retain_until_millis, None)?;
    finish_start(store, identities, api_key, profile, &allocation, factory)
}

fn allocate_start(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteWorkspace,
    retain_until_millis: i64,
    selection: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    let allocation = store.allocate_remote_runtime(expected, retain_until_millis)?;
    if let Some(selection) = selection {
        store.record_remote_network_volume_selection(&allocation, selection)?;
    }
    Ok(allocation)
}

fn finish_start<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteAllocation,
    factory: impl FnOnce(
        TrustSelection,
        &InteractiveWorkerRequest,
        Option<&RunPodNetworkVolumeExpectation>,
    ) -> Result<P, RunPodWorkspaceSetupError>,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    let allocation = prepare_identity(store, identities, expected)?;
    store.record_remote_first_pin_intent(&allocation)?;
    dispatch(
        store,
        identities,
        api_key,
        profile,
        &allocation,
        Operation::Retry,
        factory,
    )
}

fn dispatch<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    api_key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteAllocation,
    operation: Operation,
    factory: impl FnOnce(
        TrustSelection,
        &InteractiveWorkerRequest,
        Option<&RunPodNetworkVolumeExpectation>,
    ) -> Result<P, RunPodWorkspaceSetupError>,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    validate_profile(&expected.workspace().state().spec.target, profile)?;
    validate_allocation(store, expected)?;
    let request = expected.recovery_request()?;
    let selection = store.load_remote_network_volume_selection(expected)?;
    let runtime = expected
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    let trust = if let Some(ssh) = &runtime.ssh {
        let worker = runtime
            .worker
            .as_ref()
            .ok_or(RemoteWorkspaceRecoveryError::InvalidObservation)?;
        TrustSelection::Retained(
            RunPodHostTrust::retained(worker, ssh).map_err(|_| RemoteWorkspaceRecoveryError::InvalidObservation)?,
        )
    } else {
        let initial = store
            .load_remote_first_pin_request(expected)?
            .ok_or(RunPodWorkspaceSetupError::FirstPinIntentUnavailable)?;
        TrustSelection::Initial(
            RunPodHostTrust::initial_task_free(api_key, &initial)
                .map_err(|_| RemoteWorkspaceRecoveryError::InvalidObservation)?,
        )
    };
    identities.recover(request.workflow_id, request.job_id, &request.ssh_public_key)?;
    let provider = factory(trust, &request, selection.as_ref())?;
    match operation {
        Operation::Retry => retry_remote_workspace_setup(store, identities, &provider, expected),
        Operation::Recover => recover(store, identities, &provider, expected),
    }
    .map_err(Into::into)
}

/// Diagnostics contain no supplied credentials, profile payloads or private paths.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum RunPodWorkspaceSetupError {
    #[error("supplied worker profile does not match the saved target or valid setup requirements")]
    InvalidProfile,
    #[error("explicit first-pin setup intent is unavailable; missing trust cannot authorize bootstrap")]
    FirstPinIntentUnavailable,
    #[error("supplied network volume selection is invalid")]
    InvalidNetworkVolumeSelection,
    #[error("saved network volume selection cannot bind the supplied worker request and profile")]
    NetworkVolumeBindingRejected,
    #[error(transparent)]
    Setup(#[from] RemoteWorkspaceSetupError),
}

impl From<RemoteWorkspaceStoreError> for RunPodWorkspaceSetupError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::InvalidNetworkVolumeSelection => Self::InvalidNetworkVolumeSelection,
            RemoteWorkspaceStoreError::RuntimeSetupUnavailable => Self::FirstPinIntentUnavailable,
            _ => Self::Setup(error.into()),
        }
    }
}

impl From<RemoteWorkspaceRecoveryError> for RunPodWorkspaceSetupError {
    fn from(error: RemoteWorkspaceRecoveryError) -> Self {
        Self::Setup(error.into())
    }
}

impl From<RemoteSshIdentityError> for RunPodWorkspaceSetupError {
    fn from(error: RemoteSshIdentityError) -> Self {
        Self::Setup(error.into())
    }
}

#[cfg(test)]
mod tests;
