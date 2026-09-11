//! Explicit initial setup and interrupted-setup retry, separate from client reconnect.
//! All operations are synchronous and must run off the render thread.

mod runpod;
pub use runpod::{
    RunPodWorkspaceSetupError, recover_runpod_workspace, retry_runpod_workspace_setup,
    start_task_free_runpod_workspace, start_task_free_runpod_workspace_with_network_volume,
};

use crate::{
    cloud_run::{
        CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, StoredRemoteWorkspace,
        interactive_worker::InteractiveWorkerProvider,
    },
    remote_ssh_identity::{RemoteSshIdentityError, RemoteSshIdentityStore},
    remote_workspace_recovery::{RemoteWorkspaceRecoveryError, inspect_remote_allocation},
};

/// Allocate once for an explicitly requested new worker, then retain its request before ensure.
/// The provider must use this same store's durable creation fence; setup admission is not a grant.
/// Failures preserve the allocated generation for explicit retry or non-creating recovery.
/// This does not start tasks or authorize attachment. The coordinator issues no cleanup;
/// the provider's explicit ensure operation retains its existing cleanup contract.
/// # Errors
/// Rejects unsupported identity platforms, provider/ownership drift, active runtimes,
/// invalid setup retention, private-key failures and provider/observation failures.
pub fn start_remote_workspace<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    expected: &StoredRemoteWorkspace,
    retain_until_millis: i64,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
    if !cfg!(target_os = "linux") {
        return Err(RemoteSshIdentityError::UnsupportedPlatform.into());
    }
    if expected.state().spec.target.provider != provider.provider() {
        return Err(RemoteWorkspaceRecoveryError::ProviderMismatch.into());
    }
    let allocation = store.allocate_remote_runtime(expected, retain_until_millis)?;
    retry_remote_workspace_setup(store, identities, provider, &allocation)
}

/// Resume only this exact allocation after an explicit setup retry action.
/// A claimed/observed/expired setup uses non-creating recovery, never another ensure.
/// Only a still-unclaimed setup with no saved client identity may prepare a key.
/// A reserved identity must recover its existing private key without substitution.
/// Concurrent callers are arbitrated by the store CAS and provider's durable fence.
/// # Errors
/// Rejects stale/corrupt ownership, missing or mismatched keys, pending management,
/// unavailable providers and invalid observations. The coordinator issues no compensating
/// stop/delete calls; an admitted provider ensure retains its existing cleanup contract.
pub fn retry_remote_workspace_setup<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
    if expected.workspace().state().spec.target.provider != provider.provider() {
        return Err(RemoteWorkspaceRecoveryError::ProviderMismatch.into());
    }
    if !setup_available(store, expected)? {
        return recover(store, identities, provider, expected);
    }
    let allocation = match prepare_identity(store, identities, expected) {
        Ok(allocation) => allocation,
        Err(error) => return recover_preparation_failure(store, identities, provider, expected, error),
    };
    // Key generation/inspection may take time. Recheck both owned snapshots before ensure.
    if !setup_available(store, &allocation)? {
        return recover(store, identities, provider, &allocation);
    }
    let request = allocation.worker_request()?;
    let observed = provider
        .ensure_worker(&request)
        .map_err(|_| RemoteWorkspaceSetupError::ProviderUnavailable)?;
    store
        .record_remote_worker_recovery(&allocation, Some(observed.status()))
        .map_err(Into::into)
}

fn recover_preparation_failure<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
    error: RemoteWorkspaceSetupError,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
    if setup_available(store, expected)? {
        Err(error)
    } else {
        recover(store, identities, provider, expected)
    }
}

fn setup_available(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteAllocation,
) -> Result<bool, RemoteWorkspaceSetupError> {
    match store.validate_unclaimed_remote_setup(expected) {
        Ok(()) => Ok(true),
        Err(RemoteWorkspaceStoreError::RuntimeSetupUnavailable) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn prepare_identity(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
    let runtime = expected
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if let Some(public_key) = &runtime.ssh_public_key {
        identities.recover(runtime.workflow_id, runtime.job_id, public_key)?;
        return Ok(expected.clone());
    }
    let identity = identities.prepare_new(runtime.workflow_id, runtime.job_id)?;
    store
        .reserve_remote_worker_request(expected, identity.public_key())
        .map_err(Into::into)
}

fn recover<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
    validate_allocation(store, expected)?;
    let recovered = inspect_remote_allocation(identities, provider, expected)?;
    store
        .record_remote_worker_recovery(expected, recovered.observation())
        .map_err(Into::into)
}

fn validate_allocation(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteAllocation,
) -> Result<(), RemoteWorkspaceSetupError> {
    let workspace = expected.workspace();
    let current = store
        .load_remote_allocation(workspace.session_id(), &workspace.state().spec.workspace_local_id)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    if current != *expected {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    Ok(())
}

/// Private identity, provider payloads and storage details never appear in setup diagnostics.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum RemoteWorkspaceSetupError {
    #[error("remote workspace already has an allocation; recover it or explicitly retry its setup")]
    RuntimeAlreadyActive,
    #[error("remote setup authorization must end after its valid creation timestamp")]
    InvalidAllocationRetention,
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Identity(#[from] RemoteSshIdentityError),
    #[error("remote worker setup request failed; retain the allocation and use recovery before retrying")]
    ProviderUnavailable,
}

impl From<RemoteWorkspaceStoreError> for RemoteWorkspaceSetupError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::RuntimeAlreadyActive => Self::RuntimeAlreadyActive,
            RemoteWorkspaceStoreError::InvalidAllocationRetention => Self::InvalidAllocationRetention,
            _ => Self::Recovery(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
