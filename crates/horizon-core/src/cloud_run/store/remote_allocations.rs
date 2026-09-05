//! Atomic allocation of one session-owned runtime and one single-worker workflow.
//! No provider calls or creation grants occur here. Run these synchronous operations
//! off the render thread; only the durable creation fence may grant provider creation.

mod binding;
mod guards;

pub(super) use guards::{
    ensure_unbound_workflow, validate_creation_claim, validate_workflow_replacement, validate_workspace_write,
};

use super::{
    CloudStoreError, CloudWorkflowStore, MAX_RECOVERED_WORKFLOWS, PreparedWorkflowInsert,
    RemoteWorkspaceStoreError as Error, StoredRemoteWorkspace, StoredWorkflow, check_recovery_budget,
    current_unix_millis,
    database::ensure_current_schema,
    remote_workspaces::{WorkspaceReplacement, load_owned, validate_key},
};
use crate::cloud_run::{
    CLOUD_RUN_PROTOCOL_VERSION, CloudJobId, CloudJobState, CloudProgress, CloudWorkflow, CloudWorkflowId, RetryPolicy,
    WorkflowNode, WorkflowNodeKind,
};
use crate::remote_workspace::{RemoteRuntimeGeneration, RemoteRuntimePhase, RemoteWorkspaceState};
use rusqlite::{TransactionBehavior, params};

/// A recovered relationship, not permission to create or attach a provider worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRemoteAllocation {
    workspace: StoredRemoteWorkspace,
    workflow: StoredWorkflow,
}

impl StoredRemoteAllocation {
    #[must_use]
    pub fn workspace(&self) -> &StoredRemoteWorkspace {
        &self.workspace
    }

    #[must_use]
    pub fn workflow(&self) -> &StoredWorkflow {
        &self.workflow
    }
}

impl CloudWorkflowStore {
    /// Allocate a fresh generation, workflow, and ownership binding in one transaction.
    /// A competing/stale caller must reload; this never adopts an unbound active snapshot.
    /// No worker is created and no one-shot creation grant is consumed by allocation.
    /// Workflow retention bounds setup authorization, not worker execution. It must
    /// end after the store's current clock; expiry preserves non-creating recovery.
    /// Panel intent count never owns the runtime lifetime or permission to allocate.
    /// No retirement/rebinding API is exposed here. Re-claims are only meaningful
    /// while still provisioning; later observations require non-creating reconciliation.
    /// # Errors
    /// Rejects active workspaces, invalid retention, stale state, capacity limits,
    /// exhausted counters, incompatible schemas, broken bindings, or storage failures.
    pub fn allocate_remote_runtime(
        &self,
        expected: &StoredRemoteWorkspace,
        retain_until_millis: i64,
    ) -> Result<StoredRemoteAllocation, Error> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let now_millis = current_unix_millis()?;
        let PreparedAllocation { next, workflow } =
            prepare_allocation(expected.state(), now_millis, retain_until_millis)?;
        let replacement = WorkspaceReplacement::new(expected, &next)?;
        let prepared_workflow = PreparedWorkflowInsert::new(&workflow)?;
        let workspace = replacement.persist(&transaction)?;
        ensure_workflow_budget(&transaction, prepared_workflow.snapshot_len(), now_millis)?;
        let stored_workflow = prepared_workflow.persist(&transaction)?;
        let runtime = next.runtime.as_ref().ok_or(CloudStoreError::InvalidRemoteAllocation)?;
        let generation = i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?;
        transaction.execute(
            "INSERT INTO remote_runtime_allocations (workspace_local_id, session_id, generation, workflow_id, job_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                next.spec.workspace_local_id,
                expected.session_id(),
                generation,
                workflow.id.to_string(),
                runtime.job_id.to_string()
            ],
        )?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: stored_workflow,
        })
    }

    /// Recover both sides of a committed allocation from one consistent read snapshot.
    /// Unbound active records fail closed and remain available through the record-store API.
    /// Expired setup workflows remain recoverable without granting new creation.
    /// Recovery does not stop/delete workers, clear intent, or change runtime identity.
    /// # Errors
    /// Rejects invalid ownership, unbound active snapshots, corrupt relationships, or storage errors.
    pub fn load_remote_allocation(
        &self,
        session_id: &str,
        workspace_local_id: &str,
    ) -> Result<Option<StoredRemoteAllocation>, Error> {
        validate_key(session_id, workspace_local_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let workspace = load_owned(&transaction, session_id, workspace_local_id)?;
        let row = binding::load_workspace(&transaction, workspace_local_id)?;
        match (workspace, row) {
            (Some(workspace), Some(row)) => {
                let allocation = binding::recover(&transaction, &row)?;
                if allocation.workspace != workspace {
                    return Err(CloudStoreError::InvalidRemoteAllocation.into());
                }
                Ok(Some(allocation))
            }
            (Some(workspace), None) if workspace.state().runtime.is_some() => Err(Error::UnboundRuntime),
            (_, Some(_)) => Err(CloudStoreError::InvalidRemoteAllocation.into()),
            (_, None) => Ok(None),
        }
    }
}

struct PreparedAllocation {
    next: RemoteWorkspaceState,
    workflow: CloudWorkflow,
}

fn ensure_workflow_budget(
    connection: &rusqlite::Connection,
    next_bytes: usize,
    now_millis: i64,
) -> Result<(), CloudStoreError> {
    let limit = i64::try_from(MAX_RECOVERED_WORKFLOWS + 1).map_err(|_| CloudStoreError::RecoveryLimitExceeded)?;
    let mut statement = connection.prepare(
        "SELECT length(snapshot) FROM cloud_workflows INDEXED BY cloud_workflows_retention
         WHERE retain_until_millis >= ?1 LIMIT ?2",
    )?;
    let sizes = statement.query_map(params![now_millis, limit], |row| row.get::<_, i64>(0))?;
    let mut count = 1;
    let mut bytes = next_bytes;
    check_recovery_budget(count, bytes)?;
    for size in sizes {
        let size = usize::try_from(size?).map_err(|_| CloudStoreError::RecoveryLimitExceeded)?;
        bytes = bytes.checked_add(size).ok_or(CloudStoreError::RecoveryLimitExceeded)?;
        count += 1;
        check_recovery_budget(count, bytes)?;
    }
    Ok(())
}

fn prepare_allocation(
    state: &RemoteWorkspaceState,
    now_millis: i64,
    retain_until_millis: i64,
) -> Result<PreparedAllocation, Error> {
    state.validate()?;
    if state.runtime.is_some() {
        return Err(Error::RuntimeAlreadyActive);
    }
    if now_millis < 0 || retain_until_millis <= now_millis {
        return Err(Error::InvalidAllocationRetention);
    }
    let generation = state
        .spec
        .generation
        .checked_add(1)
        .filter(|generation| i64::try_from(*generation).is_ok())
        .ok_or(Error::GenerationExhausted)?;
    let workflow_id = CloudWorkflowId::new();
    let job_id = CloudJobId::new();
    let mut next = state.clone();
    next.spec.generation = generation;
    next.runtime = Some(RemoteRuntimeGeneration {
        workspace_local_id: state.spec.workspace_local_id.clone(),
        generation,
        workflow_id,
        job_id,
        phase: RemoteRuntimePhase::Provisioning,
        worker: None,
        ssh: None,
        cleanup: None,
    });
    let workflow = CloudWorkflow {
        protocol_version: CLOUD_RUN_PROTOCOL_VERSION,
        id: workflow_id,
        title: "Remote workspace".into(),
        created_at_millis: now_millis,
        updated_at_millis: now_millis,
        retain_until_millis,
        nodes: vec![WorkflowNode {
            id: job_id,
            logical_key: "workspace-worker".into(),
            label: "Workspace worker".into(),
            kind: WorkflowNodeKind::RemoteWorkspace,
            state: CloudJobState::Queued,
            outcome: None,
            progress: CloudProgress::Pending,
            weight: 1,
            attempt: 1,
            retry: RetryPolicy::default(),
            supersedes: None,
            depends_on: Vec::new(),
            source: Some(state.spec.repository.clone()),
            worker: Some(state.spec.target.clone()),
            input_artifact_ids: Vec::new(),
            outputs: Vec::new(),
            approval: None,
            release: None,
            environment_lease: None,
        }],
    };
    Ok(PreparedAllocation { next, workflow })
}

#[cfg(test)]
mod tests;
