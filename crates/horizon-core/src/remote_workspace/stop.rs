//! Explicit, durable Stop coordination. Client lifecycle never invokes this operation.

mod configured;
mod configured_azure;
mod configured_confirmation;
mod configured_runpod;
mod confirmation;

pub use configured::{ConfiguredStopError, stop_configured_remote_environment};
pub use configured_azure::{ConfiguredAzureStopError, stop_configured_azure_environment};
pub use configured_confirmation::{
    ConfiguredStopConfirmation, ConfiguredStopConfirmationError, confirm_configured_remote_environment_stop,
};
pub use configured_runpod::{ConfiguredRunPodStopError, stop_configured_runpod_environment};
pub use confirmation::{RemoteWorkspaceStopConfirmation, confirm_remote_workspace_stop};

use crate::{
    cloud_run::{
        CloudStoreError, CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, StoredRemoteWorkspace,
        WorkerLifetime,
        interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider},
    },
    remote_workspace::RemoteRuntimePhase,
};

/// Record explicit Stop intent, stop only the retained worker, then record verified completion.
/// Requires current owned workspace/workflow snapshots and an already retained exact persistent worker.
/// Timed creation/expiry cleanup needs separate coordination before durable Stop can support it.
/// No private key, allocation, reconciliation, restart, deletion or fallback is attempted.
/// Provider failures/absence preserve intent and identity for explicit retry by a fresh client.
/// Stopped is saved point-in-time state, not a checkpoint or proof of current provider state.
/// This synchronous operation must run off the render thread, only after explicit Stop admission.
/// # Errors
/// Rejects stale/unbound ownership, missing worker identity, competing management intent,
/// provider mismatch/failure, absence and unverified completion. Errors redact provider payloads.
pub fn stop_remote_workspace<P: InteractiveWorkerStopProvider + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteWorkspace,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceStopError> {
    use RemoteWorkspaceStopError as Error;
    let allocation = store
        .load_remote_allocation(expected.session_id(), &expected.state().spec.workspace_local_id)?
        .ok_or(Error::MissingAllocation)?;
    if allocation.workspace() != expected {
        return Err(Error::StateChanged);
    }
    stop_allocation(store, provider, &allocation)
}

// Configured admission must not reload/adopt a newer workflow after checking credentials.
fn stop_allocation<P: InteractiveWorkerStopProvider + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RemoteWorkspaceStopError> {
    use RemoteWorkspaceStopError as Error;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?;
    let worker = runtime.worker.as_ref().ok_or(Error::MissingWorker)?;
    if !worker.is_valid_for(provider.provider()) {
        return Err(Error::ProviderMismatch);
    }
    if worker.target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::UnsupportedLifetime);
    }
    if runtime.cleanup.is_some() {
        return Err(Error::ManagementConflict);
    }
    let now_millis = current_millis()?;
    let requested_at_millis = runtime.phase.stop_requested_at_millis().unwrap_or(now_millis);
    if requested_at_millis > now_millis {
        return Err(Error::InvalidTimestamp);
    }
    let phase = if matches!(runtime.phase, RemoteRuntimePhase::Stopped { .. }) {
        runtime.phase
    } else {
        RemoteRuntimePhase::Stopping { requested_at_millis }
    };
    let stopping = store.record_remote_stop_phase(allocation, phase)?;
    match provider.stop_worker(worker).map_err(|_| Error::ProviderUnavailable)? {
        InteractiveWorkerStop::AlreadyAbsent => return Err(Error::ResourceAbsent),
        InteractiveWorkerStop::Stopped => {}
    }
    let completed = if matches!(phase, RemoteRuntimePhase::Stopped { .. }) {
        phase
    } else {
        let observed_at_millis = current_millis()?;
        if observed_at_millis < requested_at_millis {
            return Err(Error::InvalidTimestamp);
        }
        RemoteRuntimePhase::Stopped {
            requested_at_millis,
            observed_at_millis,
        }
    };
    store.record_remote_stop_phase(&stopping, completed)
}

fn current_millis() -> Result<i64, RemoteWorkspaceStopError> {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .ok()
        .filter(|millis| *millis >= 0)
        .ok_or(RemoteWorkspaceStopError::InvalidTimestamp)
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteWorkspaceStopError {
    #[error("no current owned allocation is available to stop")]
    MissingAllocation,
    #[error("saved environment has no exact retained worker to stop")]
    MissingWorker,
    #[error("saved environment has no existing Stop intent to confirm")]
    MissingStopIntent,
    #[error("Stop observation requires an already retained complete SSH pin")]
    MissingTrust,
    #[error("saved environment changed; refresh before explicitly retrying Stop")]
    StateChanged,
    #[error("Stop provider does not match the saved worker")]
    ProviderMismatch,
    #[error("durable workspace Stop requires a persistent execution policy")]
    UnsupportedLifetime,
    #[error("existing management intent does not permit this Stop request")]
    ManagementConflict,
    #[error("worker Stop could not be verified; saved intent and identity remain retained")]
    ProviderUnavailable,
    #[error("worker is absent; Stop cannot certify retained data, and saved intent remains")]
    ResourceAbsent,
    #[error("Stop timestamp could not be safely recorded")]
    InvalidTimestamp,
    #[error("owned remote Stop state could not be safely accessed")]
    StorageUnavailable,
}

impl From<RemoteWorkspaceStoreError> for RemoteWorkspaceStopError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::RevisionConflict { .. } | RemoteWorkspaceStoreError::SnapshotConflict => {
                Self::StateChanged
            }
            _ => Self::StorageUnavailable,
        }
    }
}

impl From<CloudStoreError> for RemoteWorkspaceStopError {
    fn from(_: CloudStoreError) -> Self {
        Self::StorageUnavailable
    }
}

impl From<rusqlite::Error> for RemoteWorkspaceStopError {
    fn from(_: rusqlite::Error) -> Self {
        Self::StorageUnavailable
    }
}

#[cfg(test)]
mod tests;
