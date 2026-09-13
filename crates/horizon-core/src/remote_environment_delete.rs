//! Explicit destructive intent and exact worker-absence confirmation, never client cleanup.

use crate::{
    cloud_run::{
        CloudStoreError, CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::InteractiveWorker,
        interactive_worker_delete::{InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation},
    },
    remote_workspace::RemoteRuntimePhase,
};

/// Saved state after a deletion request or its non-mutating confirmation.
#[derive(Debug, Eq, PartialEq)]
pub struct RemoteEnvironmentDeletion {
    pub allocation: StoredRemoteAllocation,
    /// Only an exact provider absence observation permits this to be true.
    pub absence_verified: bool,
}

/// Delete the exact retained worker after an explicit destructive confirmation.
///
/// The caller must disclose running-task loss, unpushed-data loss and the provider's
/// complete deletion scope. Independent network storage is not implicitly deleted.
/// This function durably records intent before one provider call, then makes one
/// read-only absence observation. Any ambiguous response retains intent. Checking
/// never replays Delete; another attempt requires separately confirmed explicit retry.
/// The allocation, original identity and creation fence survive as a tombstone.
/// Run off the render thread; closing a view or client must never invoke this API.
/// # Errors
/// Refuses stale ownership, missing retained identity, unsupported lifetime, active
/// management intent and storage errors. Provider failures are redacted and retain intent.
pub fn delete_remote_environment<P: InteractiveWorkerDeleteObserver + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteEnvironmentDeletion, RemoteEnvironmentDeleteError> {
    let allocation = current(store, provider, expected)?;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?;
    if runtime.cleanup.is_some()
        || !matches!(
            runtime.phase,
            RemoteRuntimePhase::Ready
                | RemoteRuntimePhase::Reconciling
                | RemoteRuntimePhase::Failed
                | RemoteRuntimePhase::Stopped { .. }
        )
    {
        return Err(Error::ManagementConflict);
    }
    let requested_at_millis = current_millis()?;
    let requested =
        store.record_remote_delete_phase(&allocation, RemoteRuntimePhase::DeleteRequested { requested_at_millis })?;
    let worker = retained_worker(&requested)?;
    // Accepted, completed and lost replies all need an independent observation.
    // No provider payload enters errors or durable state, and the call is not retried.
    let _response = provider.delete_worker(worker);
    observe(store, provider, &requested)
}

/// Check previously saved Delete intent without requesting deletion or touching SSH.
/// A surviving resource remains pending, even if the provider says it is deleting.
/// Confirmed tombstones are returned without further I/O; their observation is historical.
/// # Errors
/// Refuses missing intent, stale identity, provider errors and invalid timestamps.
pub fn confirm_remote_environment_deletion<P: InteractiveWorkerDeleteObserver + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteEnvironmentDeletion, RemoteEnvironmentDeleteError> {
    let allocation = current(store, provider, expected)?;
    let phase = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?
        .phase;
    match phase {
        RemoteRuntimePhase::Deleted { .. } => Ok(RemoteEnvironmentDeletion {
            allocation,
            absence_verified: true,
        }),
        RemoteRuntimePhase::DeleteRequested { .. } => observe(store, provider, &allocation),
        _ => Err(Error::MissingDeleteIntent),
    }
}

/// Retry saved Delete intent only after a new explicit destructive confirmation.
/// Uses the same disclosure and exact resource scope as [`delete_remote_environment`].
/// First check ownership and absence without mutation: absence completes the intent
/// without another Delete; failed observation refuses dispatch. A surviving resource
/// permits one request after a revision-guarded durable retry admission. The original
/// intent time and identities remain unchanged. Refresh/reconnect must never call this.
/// Present does not prove the prior request failed: confirmation must disclose that
/// the earlier deletion may still be in progress when another request is authorized.
/// # Errors
/// Refuses missing pending intent, stale identity, provider observation errors and
/// storage conflicts. Unverified results keep intent; retry is never automatic.
pub fn retry_remote_environment_deletion<P: InteractiveWorkerDeleteObserver + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteEnvironmentDeletion, RemoteEnvironmentDeleteError> {
    let allocation = current(store, provider, expected)?;
    let phase = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?
        .phase;
    if !matches!(phase, RemoteRuntimePhase::DeleteRequested { .. }) {
        return Err(Error::MissingDeleteIntent);
    }
    let checked = observe(store, provider, &allocation)?;
    if checked.absence_verified {
        return Ok(checked);
    }
    // Even unchanged intent advances the revision: another caller using this same
    // snapshot cannot dispatch a second retry. The provider revalidates ownership.
    let admitted = store.record_remote_delete_phase(&checked.allocation, phase)?;
    let _response = provider.delete_worker(retained_worker(&admitted)?);
    observe(store, provider, &admitted)
}

fn current<P: InteractiveWorkerDeleteObserver + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, Error> {
    let allocation = store
        .load_remote_allocation(
            expected.workspace().session_id(),
            &expected.workspace().state().spec.workspace_local_id,
        )?
        .ok_or(Error::MissingAllocation)?;
    if allocation != *expected {
        return Err(Error::StateChanged);
    }
    let worker = retained_worker(&allocation)?;
    if !worker.is_valid_for(provider.provider()) {
        return Err(Error::ProviderMismatch);
    }
    if worker.target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::UnsupportedLifetime);
    }
    Ok(allocation)
}

fn retained_worker(allocation: &StoredRemoteAllocation) -> Result<&InteractiveWorker, Error> {
    allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?
        .worker
        .as_ref()
        .ok_or(Error::MissingWorker)
}

fn observe<P: InteractiveWorkerDeleteObserver + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteEnvironmentDeletion, Error> {
    let runtime = expected
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?;
    let RemoteRuntimePhase::DeleteRequested { requested_at_millis } = runtime.phase else {
        return Err(Error::MissingDeleteIntent);
    };
    if requested_at_millis > current_millis()? {
        return Err(Error::InvalidTimestamp);
    }
    let observation = provider
        .observe_worker_deletion(retained_worker(expected)?)
        .map_err(|_| Error::ProviderUnavailable)?;
    if observation == InteractiveWorkerDeletionObservation::Present {
        // Even a pending result must still belong to the exact snapshot observed.
        let allocation = current(store, provider, expected)?;
        return Ok(RemoteEnvironmentDeletion {
            allocation,
            absence_verified: false,
        });
    }
    let observed_at_millis = current_millis()?;
    if observed_at_millis < requested_at_millis {
        return Err(Error::InvalidTimestamp);
    }
    let allocation = store.record_remote_delete_phase(
        expected,
        RemoteRuntimePhase::Deleted {
            requested_at_millis,
            observed_at_millis,
        },
    )?;
    Ok(RemoteEnvironmentDeletion {
        allocation,
        absence_verified: true,
    })
}

fn current_millis() -> Result<i64, Error> {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .ok()
        .filter(|millis| *millis >= 0)
        .ok_or(Error::InvalidTimestamp)
}

type Error = RemoteEnvironmentDeleteError;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteEnvironmentDeleteError {
    #[error("no exact owned allocation is available for deletion")]
    MissingAllocation,
    #[error("no retained worker identity is available for deletion")]
    MissingWorker,
    #[error("saved environment changed; refresh before proceeding")]
    StateChanged,
    #[error("deletion provider does not match the saved worker")]
    ProviderMismatch,
    #[error("explicit environment deletion requires a persistent worker")]
    UnsupportedLifetime,
    #[error("existing management intent prevents a new Delete; check saved intent instead")]
    ManagementConflict,
    #[error("no saved explicit Delete intent is available to check")]
    MissingDeleteIntent,
    #[error("worker absence could not be verified; saved Delete intent and identity remain")]
    ProviderUnavailable,
    #[error("deletion timestamps could not be safely recorded")]
    InvalidTimestamp,
    #[error("owned remote deletion state could not be safely accessed")]
    StorageUnavailable,
}

impl From<RemoteWorkspaceStoreError> for Error {
    fn from(value: RemoteWorkspaceStoreError) -> Self {
        match value {
            RemoteWorkspaceStoreError::SnapshotConflict | RemoteWorkspaceStoreError::RevisionConflict { .. } => {
                Self::StateChanged
            }
            RemoteWorkspaceStoreError::RuntimeDeleteCoordinationRequired => Self::ManagementConflict,
            _ => Self::StorageUnavailable,
        }
    }
}

impl From<CloudStoreError> for Error {
    fn from(_: CloudStoreError) -> Self {
        Self::StorageUnavailable
    }
}

impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::StorageUnavailable
    }
}

#[cfg(test)]
mod tests;
