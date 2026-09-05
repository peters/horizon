//! Keep generic workflow/record APIs from bypassing committed runtime ownership.

use super::super::{decode_workflow_row, workflow_row};
use super::{CloudJobId, CloudStoreError, CloudWorkflow, RemoteRuntimePhase, WorkflowNodeKind, binding};
use crate::remote_workspace::RemoteWorkspaceState;
use rusqlite::{Connection, params};

pub(in crate::cloud_run::store) fn ensure_unbound_workflow(workflow: &CloudWorkflow) -> Result<(), CloudStoreError> {
    if workflow
        .nodes
        .iter()
        .any(|node| node.kind == WorkflowNodeKind::RemoteWorkspace)
    {
        return Err(CloudStoreError::RemoteAllocationRequired);
    }
    Ok(())
}

pub(in crate::cloud_run::store) fn validate_workflow_replacement(
    connection: &Connection,
    previous: &CloudWorkflow,
    next: &CloudWorkflow,
) -> Result<(), CloudStoreError> {
    if let Some(row) = binding::load_workflow(connection, &previous.id.to_string())? {
        let allocation = binding::recover(connection, &row)?;
        if next.retain_until_millis != previous.retain_until_millis {
            return Err(CloudStoreError::InvalidRemoteAllocation);
        }
        row.validate_workflow(allocation.workspace.state(), next)
    } else {
        validate_unbound_snapshot(previous)?;
        ensure_unbound_workflow(next)
    }
}

pub(in crate::cloud_run::store) fn validate_workspace_write(
    connection: &Connection,
    owner: &str,
    previous: Option<&RemoteWorkspaceState>,
    next: &RemoteWorkspaceState,
) -> Result<(), CloudStoreError> {
    if let Some(row) = binding::load_workspace(connection, &next.spec.workspace_local_id)? {
        let allocation = binding::recover(connection, &row)?;
        row.validate_workspace(owner, next)?;
        row.validate_workflow(next, allocation.workflow.workflow())?;
    } else if let Some(runtime) = previous
        .and_then(|state| state.runtime.as_ref())
        .or(next.runtime.as_ref())
        && let Some(snapshot) = workflow_row(connection, &runtime.workflow_id.to_string())?
    {
        // Losing the binding cannot turn a committed remote workflow into a legacy unbound record.
        let workflow = decode_workflow_row(runtime.workflow_id, &snapshot)?;
        validate_unbound_snapshot(workflow.workflow())?;
    }
    if let Some(runtime) = &next.runtime {
        let claimed_elsewhere: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_runtime_allocations
             WHERE workspace_local_id != ?1 AND (workflow_id = ?2 OR job_id = ?3))",
            params![
                next.spec.workspace_local_id,
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string()
            ],
            |row| row.get(0),
        )?;
        if claimed_elsewhere {
            return Err(CloudStoreError::InvalidRemoteAllocation);
        }
    }
    Ok(())
}

pub(in crate::cloud_run::store) fn validate_creation_claim(
    connection: &Connection,
    workflow: &CloudWorkflow,
    job_id: CloudJobId,
) -> Result<(), CloudStoreError> {
    let Some(row) = binding::load_workflow(connection, &workflow.id.to_string())? else {
        return validate_unbound_snapshot(workflow);
    };
    let allocation = binding::recover(connection, &row)?;
    let state = allocation.workspace.state();
    let runtime = state.runtime.as_ref().ok_or(CloudStoreError::InvalidRemoteAllocation)?;
    if allocation.workflow.workflow() != workflow || runtime.job_id != job_id {
        return Err(CloudStoreError::InvalidRemoteAllocation);
    }
    if runtime.phase != RemoteRuntimePhase::Provisioning || runtime.worker.is_some() || runtime.cleanup.is_some() {
        return Err(CloudStoreError::ClaimTargetNotReady(job_id));
    }
    Ok(())
}

fn validate_unbound_snapshot(workflow: &CloudWorkflow) -> Result<(), CloudStoreError> {
    ensure_unbound_workflow(workflow).map_err(|_| CloudStoreError::InvalidRemoteAllocation)
}
