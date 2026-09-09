//! Explicit supplied-profile setup; missing trust never authorizes first-pin bootstrap.

use super::{
    CloudWorkflowStore, InteractiveWorkerProvider, RemoteSshIdentityError, RemoteSshIdentityStore,
    RemoteWorkspaceRecoveryError, RemoteWorkspaceSetupError, RemoteWorkspaceStoreError, StoredRemoteAllocation,
    StoredRemoteWorkspace, prepare_identity, recover, retry_remote_workspace_setup, validate_allocation,
};
use crate::cloud_run::{
    WorkerTarget,
    runpod::{
        RunPodApiKey, RunPodClient, RunPodHostTrust, RunPodInteractiveWorkerProvider, RunPodProfile, validate_target,
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
        |trust| provider(store, api_key, profile, trust),
    )
}

/// Retry this exact setup using positive initial intent or the complete retained pin.
/// Never prepares a replacement key or records new intent. Claimed, observed or
/// expired setup uses non-creating recovery. Success remains Reconciling, not ready.
/// Supplied profile validation does not detect historical edits under the same name.
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
        |trust| provider(store, api_key, profile, trust),
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
        |trust| provider(store, api_key, profile, trust),
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
) -> RunPodInteractiveWorkerProvider {
    let (TrustSelection::Initial(trust) | TrustSelection::Retained(trust)) = trust;
    RunPodInteractiveWorkerProvider::new(RunPodClient::new(api_key, store.clone()), profile.clone(), trust)
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
    factory: impl FnOnce(TrustSelection) -> P,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    validate_profile(&expected.state().spec.target, profile)?;
    let allocation = store.allocate_remote_runtime(expected, retain_until_millis)?;
    let allocation = prepare_identity(store, identities, &allocation)?;
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
    factory: impl FnOnce(TrustSelection) -> P,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    validate_profile(&expected.workspace().state().spec.target, profile)?;
    validate_allocation(store, expected)?;
    let request = expected.recovery_request()?;
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
    let provider = factory(trust);
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
    #[error(transparent)]
    Setup(#[from] RemoteWorkspaceSetupError),
}

impl From<RemoteWorkspaceStoreError> for RunPodWorkspaceSetupError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
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
