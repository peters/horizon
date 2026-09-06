//! Read-only provider observations for the global remote-environment overview.
//! Synchronous provider/store operations must run off the render thread.

mod configured;

pub use configured::{ConfiguredObservationError, observe_configured_remote_environment};

use crate::{
    cloud_run::{
        CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, StoredRemoteWorkspace,
        interactive_worker::{InteractiveWorkerIdentity, InteractiveWorkerLifecycle, InteractiveWorkerProvider},
    },
    remote_workspace::RemoteEnvironmentSummary,
};

/// Saved metadata and a point-in-time provider observation, not lifecycle authority.
/// Neither this result nor its timestamp certifies continued freshness or attachment readiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEnvironmentObservation {
    pub saved: RemoteEnvironmentSummary,
    pub observed_at_millis: i64,
    /// `None` means no exact worker was found. Saved identity remains in `saved`.
    /// Absence never authorizes replacement, removal from inventory or cleanup.
    pub worker: Option<ObservedRemoteWorker>,
}

impl RemoteEnvironmentObservation {
    /// Absolute UTC timestamp for a cached point-in-time observation label.
    #[must_use]
    pub fn observed_at_rfc3339(&self) -> Option<String> {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.observed_at_millis) * 1_000_000)
            .ok()?
            .format(&time::format_description::well_known::Rfc3339)
            .ok()
    }
}

/// Overview-safe projection. No task payload, SSH coordinates or key material is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedRemoteWorker {
    pub identity: InteractiveWorkerIdentity,
    /// Worker readiness is not workspace readiness or proof of retained task/repository state.
    pub lifecycle: InteractiveWorkerLifecycle,
}

/// Inspect a retained exact worker, or non-creatively reconcile its reserved public request.
/// Requires matching owned workspace/workflow snapshots around provider I/O. No private
/// identity is opened, no store record is changed, and no create/stop/delete occurs.
/// Pending management remains observable without changing or cancelling its intent.
/// Provider implementations must bound read operations; invoke this off the render thread.
/// # Errors
/// Rejects unbound/missing requests, stale ownership, provider errors and mismatched
/// resource/target/key/pin/lifetime observations. Errors redact provider and task payloads.
pub fn observe_remote_environment<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteWorkspace,
) -> Result<RemoteEnvironmentObservation, RemoteEnvironmentObservationError> {
    let allocation = load_current(store, expected)?;
    let request = allocation.worker_request()?;
    if !request.is_valid_for(provider.provider()) {
        return Err(RemoteEnvironmentObservationError::ProviderMismatch);
    }
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteEnvironmentObservationError::MissingAllocation)?;
    let observation = match &runtime.worker {
        Some(worker) => provider.inspect_worker(worker),
        None => provider.reconcile_worker(&request),
    }
    .map_err(|_| RemoteEnvironmentObservationError::ProviderUnavailable)?;
    let current = load_current(store, expected)?;
    if current != allocation {
        return Err(RemoteEnvironmentObservationError::StateChanged);
    }
    current.validate_worker_observation(observation.as_ref())?;
    let observed_at_millis = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| RemoteEnvironmentObservationError::InvalidObservation)?;
    Ok(RemoteEnvironmentObservation {
        saved: current.workspace().environment_summary(),
        observed_at_millis,
        worker: observation.map(|status| ObservedRemoteWorker {
            identity: status.worker.identity,
            lifecycle: status.lifecycle,
        }),
    })
}

fn load_current(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteWorkspace,
) -> Result<StoredRemoteAllocation, RemoteEnvironmentObservationError> {
    let current = store
        .load_remote_allocation(expected.session_id(), &expected.state().spec.workspace_local_id)?
        .ok_or(RemoteEnvironmentObservationError::MissingAllocation)?;
    if current.workspace() != expected {
        return Err(RemoteEnvironmentObservationError::StateChanged);
    }
    Ok(current)
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum RemoteEnvironmentObservationError {
    #[error("no owned allocation is available for this saved environment")]
    MissingAllocation,
    #[error("saved environment has no retained public worker request")]
    MissingRequest,
    #[error("observation provider does not match the saved environment")]
    ProviderMismatch,
    #[error("remote environment could not be observed; no resource or saved state was changed")]
    ProviderUnavailable,
    #[error("provider observation does not match the saved environment identity")]
    InvalidObservation,
    #[error("saved environment changed during observation; refresh before retrying")]
    StateChanged,
    #[error("owned remote environment could not be safely read from storage")]
    StorageUnavailable,
}

impl From<RemoteWorkspaceStoreError> for RemoteEnvironmentObservationError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::RuntimeRequestRequired => Self::MissingRequest,
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
