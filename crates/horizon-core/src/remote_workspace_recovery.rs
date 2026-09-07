//! Non-creating recovery of an owned runtime through its retained client identity.
//! Synchronous provider/key/store operations must run off the render thread.

use crate::{
    cloud_run::{
        CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation,
        interactive_worker::{InteractiveWorkerProvider, InteractiveWorkerStatus},
    },
    remote_ssh_identity::{RemoteSshIdentity, RemoteSshIdentityError, RemoteSshIdentityStore},
};

/// A point-in-time recovery result, not a connection or permission to start tasks.
/// Attachment still needs fresh ownership/lifetime/cost-policy checks, pinned SSH
/// transport, and verified repository/task state. Dropping this value has no remote effect.
pub struct RecoveredRemoteWorkspace {
    allocation: StoredRemoteAllocation,
    identity: RemoteSshIdentity,
    observation: Option<InteractiveWorkerStatus>,
}

impl RecoveredRemoteWorkspace {
    #[must_use]
    pub fn allocation(&self) -> &StoredRemoteAllocation {
        &self.allocation
    }

    #[must_use]
    pub fn identity(&self) -> &RemoteSshIdentity {
        &self.identity
    }

    /// `None` means the provider found no exact resource, never permission to create.
    #[must_use]
    pub fn observation(&self) -> Option<&InteractiveWorkerStatus> {
        self.observation.as_ref()
    }
}

impl std::fmt::Debug for RecoveredRemoteWorkspace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecoveredRemoteWorkspace")
            .finish_non_exhaustive()
    }
}

/// Recover existing identity, inspect the saved resource (or reconcile a lost response),
/// then atomically retain the exact worker and host pin without changing runtime IDs.
/// No key preparation, allocation, creation claim, ensure, restart, stop or delete occurs.
/// An absent worker remains reconciling; errors never initiate compensating cleanup.
/// Provider implementations are responsible for bounded read operations.
/// # Errors
/// Fails closed on missing identity, ownership/snapshot drift, pending management,
/// mismatched/expired ready observations, unsupported platforms and provider/store failures.
pub fn recover_remote_workspace<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    session_id: &str,
    workspace_local_id: &str,
) -> Result<RecoveredRemoteWorkspace, RemoteWorkspaceRecoveryError> {
    let allocation = store
        .load_remote_allocation(session_id, workspace_local_id)?
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    recover_remote_allocation(store, identities, provider, &allocation)
}

pub(crate) fn recover_remote_allocation<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
) -> Result<RecoveredRemoteWorkspace, RemoteWorkspaceRecoveryError> {
    let request = allocation.recovery_request()?;
    if !request.is_valid_for(provider.provider()) {
        return Err(RemoteWorkspaceRecoveryError::ProviderMismatch);
    }
    let identity = identities.recover(request.workflow_id, request.job_id, &request.ssh_public_key)?;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteWorkspaceRecoveryError::MissingAllocation)?;
    let observation = match &runtime.worker {
        Some(worker) => provider.inspect_worker(worker),
        None => provider.reconcile_worker(&request),
    }
    .map_err(|_| RemoteWorkspaceRecoveryError::ProviderUnavailable)?;
    let allocation = store.record_remote_worker_recovery(allocation, observation.as_ref())?;
    Ok(RecoveredRemoteWorkspace {
        allocation,
        identity,
        observation,
    })
}

/// Errors redact provider responses, database detail, task content and private key paths.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum RemoteWorkspaceRecoveryError {
    #[error("no owned remote allocation is available; recovery did not allocate one")]
    MissingAllocation,
    #[error("remote workspace has no saved SSH request; recovery did not prepare one")]
    MissingRequest,
    #[error("remote workspace has pending management intent")]
    ManagementPending,
    #[error("remote workspace recovery provider does not match the saved target")]
    ProviderMismatch,
    #[error(transparent)]
    Identity(#[from] RemoteSshIdentityError),
    #[error("remote worker inspection failed; recovery did not create or delete anything")]
    ProviderUnavailable,
    #[error("remote worker observation does not match the saved request or pinned identity")]
    InvalidObservation,
    #[error("remote allocation changed during recovery; reload before retrying")]
    StateChanged,
    #[error("owned remote allocation could not be safely recovered from storage")]
    StorageUnavailable,
}

impl From<RemoteWorkspaceStoreError> for RemoteWorkspaceRecoveryError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::RuntimeRequestRequired => Self::MissingRequest,
            RemoteWorkspaceStoreError::RuntimeRecoveryUnavailable => Self::ManagementPending,
            RemoteWorkspaceStoreError::InvalidWorkerObservation => Self::InvalidObservation,
            RemoteWorkspaceStoreError::SnapshotConflict | RemoteWorkspaceStoreError::RevisionConflict { .. } => {
                Self::StateChanged
            }
            _ => Self::StorageUnavailable,
        }
    }
}

#[cfg(test)]
mod tests;
