//! Exact-snapshot Stop and Start intent transitions; provider operations remain outside storage.

use super::{CloudWorkflowStore, RemoteRuntimePhase, StoredRemoteAllocation, WorkspaceReplacement, binding};
use crate::remote_workspace::{
    start::RemoteWorkspaceStartError as StartError, stop::RemoteWorkspaceStopError as Error,
};
use rusqlite::TransactionBehavior;

impl CloudWorkflowStore {
    pub(crate) fn record_remote_stop_phase(
        &self,
        expected: &StoredRemoteAllocation,
        phase: RemoteRuntimePhase,
    ) -> Result<StoredRemoteAllocation, Error> {
        if phase.stop_requested_at_millis().is_none() {
            return Err(Error::ManagementConflict);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::super::database::ensure_current_schema(&transaction)?;
        let row = binding::load_workspace(&transaction, &expected.workspace.state().spec.workspace_local_id)?
            .ok_or(Error::MissingAllocation)?;
        let current = binding::recover(&transaction, &row)?;
        if current != *expected {
            return Err(Error::StateChanged);
        }
        let mut next = current.workspace.state().clone();
        let runtime = next.runtime.as_mut().ok_or(Error::MissingAllocation)?;
        if runtime.cleanup.is_some() || runtime.worker.is_none() {
            return Err(Error::ManagementConflict);
        }
        runtime.phase = phase;
        if next == *current.workspace.state() {
            return Ok(current);
        }
        let workspace = WorkspaceReplacement::new(&current.workspace, &next)?.persist(&transaction)?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }

    /// Record explicit Start intent over a saved Stopped record, keep it across a retry,
    /// or resolve it into a renewed observation (`Reconciling`) once the same worker was
    /// observed again. Every write is a CAS against the exact allocation snapshot; the
    /// retained worker and pin are never changed here.
    pub(crate) fn record_remote_start_phase(
        &self,
        expected: &StoredRemoteAllocation,
        phase: RemoteRuntimePhase,
    ) -> Result<StoredRemoteAllocation, StartError> {
        if phase.start_requested_at_millis().is_none() && phase != RemoteRuntimePhase::Reconciling {
            return Err(StartError::ManagementConflict);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::super::database::ensure_current_schema(&transaction)?;
        let row = binding::load_workspace(&transaction, &expected.workspace.state().spec.workspace_local_id)?
            .ok_or(StartError::MissingAllocation)?;
        let current = binding::recover(&transaction, &row)?;
        if current != *expected {
            return Err(StartError::StateChanged);
        }
        let mut next = current.workspace.state().clone();
        let runtime = next.runtime.as_mut().ok_or(StartError::MissingAllocation)?;
        if runtime.cleanup.is_some() || runtime.worker.is_none() || runtime.ssh.is_none() {
            return Err(StartError::ManagementConflict);
        }
        let permitted = match (runtime.phase, phase) {
            (
                RemoteRuntimePhase::Stopped { observed_at_millis, .. },
                RemoteRuntimePhase::Starting { requested_at_millis },
            ) => requested_at_millis >= observed_at_millis,
            (RemoteRuntimePhase::Starting { .. }, RemoteRuntimePhase::Starting { .. }) => runtime.phase == phase,
            (RemoteRuntimePhase::Starting { .. }, RemoteRuntimePhase::Reconciling) => true,
            _ => false,
        };
        if !permitted {
            return Err(StartError::NotStopped);
        }
        runtime.phase = phase;
        if next == *current.workspace.state() {
            return Ok(current);
        }
        let workspace = WorkspaceReplacement::new(&current.workspace, &next)?.persist(&transaction)?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }
}
