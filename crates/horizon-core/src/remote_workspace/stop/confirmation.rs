//! Observe an existing intent; only verified completion writes local state, never the provider.

use super::{RemoteRuntimePhase, RemoteWorkspaceStopError as Error, current_millis};
use crate::cloud_run::{
    CloudWorkflowStore, StoredRemoteAllocation, WorkerLifetime,
    interactive_worker_stop::{
        InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation, InteractiveWorkerStopObserver,
    },
};

#[derive(Debug, Eq, PartialEq)]
pub struct RemoteWorkspaceStopConfirmation {
    /// Unchanged for pending/absent observations and already-confirmed Stop.
    pub allocation: StoredRemoteAllocation,
    pub observation: InteractiveWorkerStopObservation,
}

/// Confirm only an already persisted Stop intent without reissuing any provider mutation.
/// Requires the exact complete allocation snapshot, worker and saved public host pin.
/// The observer is provider-read-only; retained-stopped proof may CAS-write local completion.
/// Pending, absence and errors preserve the original intent, identity and timestamps.
/// No private key, host-key lookup, first pin, recovery, allocation or cleanup is attempted.
/// A repeated observation of saved Stopped never rewrites its original observation time.
/// Run off the render thread; the result is point-in-time retention, not a checkpoint,
/// task success, live SSH trust verification, billing cessation or filesystem durability.
/// # Errors
/// Rejects missing intent/trust, mismatched ownership/provider, competing management,
/// snapshot or selection drift, failed observations, unsafe clocks and storage errors.
pub fn confirm_remote_workspace_stop<P: InteractiveWorkerStopObserver + ?Sized>(
    store: &CloudWorkflowStore,
    observer: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteWorkspaceStopConfirmation, Error> {
    let allocation = current(store, expected)?;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?;
    let requested_at_millis = runtime
        .phase
        .stop_requested_at_millis()
        .ok_or(Error::MissingStopIntent)?;
    if runtime.cleanup.is_some() {
        return Err(Error::ManagementConflict);
    }
    let worker = runtime.worker.as_ref().ok_or(Error::MissingWorker)?;
    if !worker.is_valid_for(observer.provider()) {
        return Err(Error::ProviderMismatch);
    }
    if worker.target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::UnsupportedLifetime);
    }
    let ssh = runtime
        .ssh
        .as_ref()
        .filter(|ssh| ssh.is_complete())
        .ok_or(Error::MissingTrust)?;
    if requested_at_millis > current_millis()? {
        return Err(Error::InvalidTimestamp);
    }
    let selection = store.load_remote_network_volume_selection(&allocation)?;
    let observation = observer.observe_worker_stop(InteractiveWorkerStopExpectation {
        worker,
        ssh,
        network_volume: selection.as_ref(),
    });
    current(store, &allocation)?;
    if store.load_remote_network_volume_selection(&allocation)? != selection {
        return Err(Error::StateChanged);
    }
    let observation = observation.map_err(|_| Error::ProviderUnavailable)?;
    let confirmed = if observation == InteractiveWorkerStopObservation::RetainedStopped
        && matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. })
    {
        let observed_at_millis = current_millis()?;
        if observed_at_millis < requested_at_millis {
            return Err(Error::InvalidTimestamp);
        }
        store.record_remote_stop_phase(
            &allocation,
            RemoteRuntimePhase::Stopped {
                requested_at_millis,
                observed_at_millis,
            },
        )?
    } else {
        allocation
    };
    Ok(RemoteWorkspaceStopConfirmation {
        allocation: confirmed,
        observation,
    })
}

fn current(store: &CloudWorkflowStore, expected: &StoredRemoteAllocation) -> Result<StoredRemoteAllocation, Error> {
    let current = store
        .load_remote_allocation(
            expected.workspace().session_id(),
            &expected.workspace().state().spec.workspace_local_id,
        )?
        .ok_or(Error::MissingAllocation)?;
    if current != *expected {
        return Err(Error::StateChanged);
    }
    Ok(current)
}
