//! Validated workflow data prepared before entering a shared write transaction.

use super::{CloudStoreError, CloudWorkflow, StoredWorkflow, encode_workflow, workflow_row};
use rusqlite::{Transaction, params};

pub(super) struct PreparedWorkflowInsert<'a> {
    workflow: &'a CloudWorkflow,
    snapshot: Vec<u8>,
}

impl<'a> PreparedWorkflowInsert<'a> {
    pub(super) fn new(workflow: &'a CloudWorkflow) -> Result<Self, CloudStoreError> {
        Ok(Self {
            workflow,
            snapshot: encode_workflow(workflow)?,
        })
    }

    /// The caller must check the current schema in an immediate write transaction.
    /// The returned value is provisional until that caller commits.
    pub(super) fn persist(&self, transaction: &Transaction<'_>) -> Result<StoredWorkflow, CloudStoreError> {
        let workflow = self.workflow;
        let id = workflow.id.to_string();
        if workflow_row(transaction, &id)?.is_some() {
            return Err(CloudStoreError::WorkflowExists(workflow.id));
        }
        transaction.execute(
            "INSERT INTO cloud_workflows (
                workflow_id, revision, created_at_millis, updated_at_millis, retain_until_millis, snapshot
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5)",
            params![
                id,
                workflow.created_at_millis,
                workflow.updated_at_millis,
                workflow.retain_until_millis,
                self.snapshot
            ],
        )?;
        Ok(StoredWorkflow {
            workflow: workflow.clone(),
            revision: 1,
        })
    }
}
