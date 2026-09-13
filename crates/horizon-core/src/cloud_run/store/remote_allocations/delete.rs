//! Dedicated Delete transitions, retaining allocation identity and its creation fence.

use super::{CloudWorkflowStore, StoredRemoteAllocation, WorkspaceReplacement, binding};
use crate::{
    remote_environment_delete::RemoteEnvironmentDeleteError as Error,
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase},
};
use rusqlite::TransactionBehavior;

impl CloudWorkflowStore {
    pub(crate) fn record_remote_delete_phase(
        &self,
        expected: &StoredRemoteAllocation,
        phase: RemoteRuntimePhase,
    ) -> Result<StoredRemoteAllocation, Error> {
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
        if let RemoteRuntimePhase::DeleteRequested { requested_at_millis } = phase {
            runtime.cleanup = Some(RemoteCleanupIntent {
                reason: RemoteCleanupReason::WorkspaceRemoved,
                requested_at_millis,
            });
        }
        runtime.phase = phase;
        let workspace = WorkspaceReplacement::for_deletion(&current.workspace, &next)?.persist(&transaction)?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }
}
