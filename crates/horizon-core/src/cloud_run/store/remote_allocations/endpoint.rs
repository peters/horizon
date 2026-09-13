//! Exact-snapshot authenticated coordinate persistence, without lifecycle transitions.

use super::{CloudWorkflowStore, StoredRemoteAllocation, WorkspaceReplacement, binding};
use rusqlite::TransactionBehavior;

impl CloudWorkflowStore {
    /// Only the authenticated refresh coordinator supplies these coordinates, after
    /// original-key proof and a matching provider re-observation. No phase is changed.
    pub(crate) fn record_remote_endpoint_refresh(
        &self,
        expected: &StoredRemoteAllocation,
        endpoint: &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    ) -> Result<StoredRemoteAllocation, crate::remote_workspace::start::RemoteEndpointRefreshError> {
        use crate::remote_workspace::start::RemoteEndpointRefreshError as RefreshError;
        let mut connection = self.connection().map_err(|_| RefreshError::StorageUnavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| RefreshError::StorageUnavailable)?;
        super::super::database::ensure_current_schema(&transaction).map_err(|_| RefreshError::StorageUnavailable)?;
        let row = binding::load_workspace(&transaction, &expected.workspace.state().spec.workspace_local_id)
            .map_err(|_| RefreshError::StorageUnavailable)?
            .ok_or(RefreshError::StateChanged)?;
        let current = binding::recover(&transaction, &row).map_err(|_| RefreshError::StorageUnavailable)?;
        if current != *expected {
            return Err(RefreshError::StateChanged);
        }
        let mut next = current.workspace.state().clone();
        next.runtime.as_mut().ok_or(RefreshError::IdentityUnavailable)?.ssh = Some(endpoint.clone());
        let replacement = WorkspaceReplacement::for_endpoint_refresh(&current.workspace, &next)
            .map_err(|_| RefreshError::ObservationChanged)?;
        if next == *current.workspace.state() {
            return Ok(current);
        }
        let workspace = replacement
            .persist(&transaction)
            .map_err(|_| RefreshError::StorageUnavailable)?;
        transaction.commit().map_err(|_| RefreshError::StorageUnavailable)?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }
}
