//! Bounded allocation lookup and cross-record identity validation.

use super::super::{decode_workflow_row, parse_workflow_id, workflow_row};
use super::{CloudStoreError, CloudWorkflow, Error, StoredRemoteAllocation, WorkflowNodeKind, load_owned};
use crate::remote_workspace::RemoteWorkspaceState;
use rusqlite::{Connection, OptionalExtension};

pub(super) struct AllocationRow {
    workspace_local_id: String,
    session_id: String,
    generation: i64,
    workflow_id: String,
    job_id: String,
}

pub(super) fn load_workspace(connection: &Connection, id: &str) -> rusqlite::Result<Option<AllocationRow>> {
    load_row(
        connection,
        "SELECT substr(workspace_local_id, 1, 129), substr(session_id, 1, 37), generation,
                substr(workflow_id, 1, 37), substr(job_id, 1, 37)
         FROM remote_runtime_allocations WHERE workspace_local_id = ?1",
        id,
    )
}

pub(super) fn load_workflow(connection: &Connection, id: &str) -> rusqlite::Result<Option<AllocationRow>> {
    load_row(
        connection,
        "SELECT substr(workspace_local_id, 1, 129), substr(session_id, 1, 37), generation,
                substr(workflow_id, 1, 37), substr(job_id, 1, 37)
         FROM remote_runtime_allocations INDEXED BY remote_runtime_allocations_workflow WHERE workflow_id = ?1",
        id,
    )
}

fn load_row(connection: &Connection, query: &str, id: &str) -> rusqlite::Result<Option<AllocationRow>> {
    connection
        .query_row(query, [id], |row| {
            Ok(AllocationRow {
                workspace_local_id: row.get(0)?,
                session_id: row.get(1)?,
                generation: row.get(2)?,
                workflow_id: row.get(3)?,
                job_id: row.get(4)?,
            })
        })
        .optional()
}

pub(super) fn recover(connection: &Connection, row: &AllocationRow) -> Result<StoredRemoteAllocation, CloudStoreError> {
    super::validate_key(&row.session_id, &row.workspace_local_id).map_err(storage_error)?;
    let workspace = load_owned(connection, &row.session_id, &row.workspace_local_id)
        .map_err(storage_error)?
        .ok_or(CloudStoreError::InvalidRemoteAllocation)?;
    row.validate_workspace(workspace.session_id(), workspace.state())?;
    let workflow_id = parse_workflow_id(&row.workflow_id)?;
    let workflow = workflow_row(connection, &row.workflow_id)?
        .ok_or(CloudStoreError::InvalidRemoteAllocation)
        .and_then(|snapshot| decode_workflow_row(workflow_id, &snapshot))?;
    row.validate_workflow(workspace.state(), workflow.workflow())?;
    Ok(StoredRemoteAllocation { workspace, workflow })
}

impl AllocationRow {
    pub(super) fn validate_workspace(&self, owner: &str, state: &RemoteWorkspaceState) -> Result<(), CloudStoreError> {
        let runtime = state.runtime.as_ref().ok_or(CloudStoreError::InvalidRemoteAllocation)?;
        if self.session_id != owner
            || self.workspace_local_id != state.spec.workspace_local_id
            || self.generation <= 0
            || u64::try_from(self.generation).ok() != Some(runtime.generation)
            || self.workflow_id != runtime.workflow_id.to_string()
            || self.job_id != runtime.job_id.to_string()
        {
            return Err(CloudStoreError::InvalidRemoteAllocation);
        }
        Ok(())
    }

    pub(super) fn validate_workflow(
        &self,
        state: &RemoteWorkspaceState,
        workflow: &CloudWorkflow,
    ) -> Result<(), CloudStoreError> {
        let [node] = workflow.nodes.as_slice() else {
            return Err(CloudStoreError::InvalidRemoteAllocation);
        };
        if self.workflow_id != workflow.id.to_string()
            || self.job_id != node.id.to_string()
            || node.kind != WorkflowNodeKind::RemoteWorkspace
            || node.source.as_ref() != Some(&state.spec.repository)
            || node.worker.as_ref() != Some(&state.spec.target)
        {
            return Err(CloudStoreError::InvalidRemoteAllocation);
        }
        Ok(())
    }
}

// Preserve retryable database errors without creating a recursive public error type.
pub(super) fn storage_error(error: Error) -> CloudStoreError {
    match error {
        Error::Storage(error) => error,
        Error::Database(error) => CloudStoreError::Database(error),
        _ => CloudStoreError::InvalidRemoteAllocation,
    }
}
