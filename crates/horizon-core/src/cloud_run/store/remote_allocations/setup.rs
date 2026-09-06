//! Setup admission is a point-in-time check, never a provider creation grant.

use super::super::{current_unix_millis, database::ensure_current_schema, validate_claim_target};
use super::{CloudWorkflowStore, Error, RemoteRuntimePhase, StoredRemoteAllocation, binding, request::CLAIM_LOOKUP};

impl CloudWorkflowStore {
    pub(crate) fn validate_unclaimed_remote_setup(&self, expected: &StoredRemoteAllocation) -> Result<(), Error> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let row = binding::load_workspace(&transaction, &expected.workspace.state().spec.workspace_local_id)?
            .ok_or(Error::UnboundRuntime)?;
        let current = binding::recover(&transaction, &row)?;
        if current != *expected {
            return Err(Error::SnapshotConflict);
        }
        let state = current.workspace.state();
        let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        if runtime.phase != RemoteRuntimePhase::Provisioning
            || runtime.worker.is_some()
            || runtime.cleanup.is_some()
            || current.workflow.workflow().retain_until_millis < current_unix_millis()?
            || validate_claim_target(current.workflow.workflow(), runtime.job_id, &state.spec.target).is_err()
            || transaction.query_row(CLAIM_LOOKUP, [runtime.workflow_id.to_string()], |row| {
                row.get::<_, bool>(0)
            })?
        {
            return Err(Error::RuntimeSetupUnavailable);
        }
        Ok(())
    }
}
