//! Setup admission is a point-in-time check, never a provider creation grant.

use super::super::{
    CloudStoreError, current_unix_millis,
    database::{ensure_current_schema, open_read_connection},
    validate_claim_target,
};
use super::{CloudWorkflowStore, Error, RemoteRuntimePhase, StoredRemoteAllocation, binding, request::CLAIM_LOOKUP};
use crate::cloud_run::{ArtifactDigest, CloudProvider, interactive_worker::InteractiveWorkerRequest};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

impl CloudWorkflowStore {
    pub(crate) fn validate_unclaimed_remote_setup(&self, expected: &StoredRemoteAllocation) -> Result<(), Error> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        validate_unclaimed(&transaction, &current)
    }

    /// Explicitly retain first-pin intent for this unclaimed, task-free `RunPod` setup.
    /// The caller selects an approved entrypoint-only image and must not run setup or
    /// tasks before the first pin is committed. The private client key must already be
    /// durable and its public request reserved. No provider or credential I/O occurs.
    /// This neither consumes a creation grant nor authorizes tasks or attachment.
    /// Run synchronously off the render thread; repeated pre-claim calls are idempotent.
    /// # Errors
    /// Rejects stale/foreign allocations, unsupported targets, missing requests, late
    /// intent, expired setup and malformed storage without changing existing snapshots.
    pub fn record_remote_first_pin_intent(&self, expected: &StoredRemoteAllocation) -> Result<(), Error> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        validate_unclaimed(&transaction, &current)?;
        let request = current.worker_request()?;
        if request.target.provider != CloudProvider::RunPod {
            return Err(Error::RuntimeSetupUnavailable);
        }
        if has_first_pin_intent(&transaction, &current, &request)? {
            return Ok(());
        }
        let state = current.workspace.state();
        let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        transaction.execute(
            "INSERT INTO remote_first_pin_intents
             (workspace_local_id, session_id, generation, workflow_id, job_id, version, request_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![
                state.spec.workspace_local_id,
                current.workspace.session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                request.workflow_id.to_string(),
                request.job_id.to_string(),
                request_digest(&request)?.as_str(),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read positive first-pin intent for this exact owned allocation and request.
    /// `Some` identifies an explicitly task-free initial setup, never creation or task
    /// authority. Claimed/provisioning attempts remain inspectable after restart or
    /// setup expiry; the provider and pin CAS still enforce ownership and lifetime.
    /// `None` requires retained trust or refusal, never a missing-pin bootstrap fallback.
    /// A saved full pin or validated schema-four store returns `None`.
    /// No database creation/migration or writes.
    /// # Errors
    /// Rejects snapshot drift, management intent, missing requests and corrupt storage.
    pub fn load_remote_first_pin_request(
        &self,
        expected: &StoredRemoteAllocation,
    ) -> Result<Option<InteractiveWorkerRequest>, Error> {
        let mut connection = open_read_connection(self.path())?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        let request = current.recovery_request()?;
        if transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? == 4 {
            return Ok(None);
        }
        let runtime = current
            .workspace
            .state()
            .runtime
            .as_ref()
            .ok_or(Error::UnboundRuntime)?;
        if runtime.ssh.is_some()
            || !matches!(
                runtime.phase,
                RemoteRuntimePhase::Provisioning | RemoteRuntimePhase::Reconciling
            )
        {
            return Ok(None);
        }
        Ok(has_first_pin_intent(&transaction, &current, &request)?.then_some(request))
    }
}

fn exact_allocation(
    connection: &Connection,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, Error> {
    let row = binding::load_workspace(connection, &expected.workspace.state().spec.workspace_local_id)?
        .ok_or(Error::UnboundRuntime)?;
    let current = binding::recover(connection, &row)?;
    if current != *expected {
        return Err(Error::SnapshotConflict);
    }
    Ok(current)
}

fn validate_unclaimed(connection: &Connection, current: &StoredRemoteAllocation) -> Result<(), Error> {
    let state = current.workspace.state();
    let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
    if runtime.phase != RemoteRuntimePhase::Provisioning
        || runtime.worker.is_some()
        || runtime.ssh.is_some()
        || runtime.cleanup.is_some()
        || current.workflow.workflow().retain_until_millis < current_unix_millis()?
        || validate_claim_target(current.workflow.workflow(), runtime.job_id, &state.spec.target).is_err()
        || connection.query_row(CLAIM_LOOKUP, [runtime.workflow_id.to_string()], |row| {
            row.get::<_, bool>(0)
        })?
    {
        return Err(Error::RuntimeSetupUnavailable);
    }
    Ok(())
}

fn request_digest(request: &InteractiveWorkerRequest) -> Result<ArtifactDigest, Error> {
    // Version-one binding: request IDs are separate columns; hash the exact target
    // and canonical reserved client key with a fixed JSON tuple representation.
    let bytes = serde_json::to_vec(&(1_u32, &request.target, &request.ssh_public_key))
        .map_err(|_| Error::InvalidStoredSnapshot)?;
    Ok(ArtifactDigest::sha256(&bytes))
}

fn has_first_pin_intent(
    connection: &Connection,
    allocation: &StoredRemoteAllocation,
    request: &InteractiveWorkerRequest,
) -> Result<bool, Error> {
    let state = allocation.workspace.state();
    let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
    let matches: Option<bool> = connection
        .query_row(
            "SELECT session_id = ?2 AND generation = ?3 AND workflow_id = ?4 AND job_id = ?5
             AND version = 1 AND request_digest = ?6
         FROM remote_first_pin_intents WHERE workspace_local_id = ?1",
            params![
                state.spec.workspace_local_id,
                allocation.workspace.session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                request.workflow_id.to_string(),
                request.job_id.to_string(),
                request_digest(request)?.as_str()
            ],
            |row| row.get(0),
        )
        .optional()?;
    match matches {
        Some(true) if request.target.provider == CloudProvider::RunPod => Ok(true),
        Some(_) => Err(CloudStoreError::InvalidRemoteAllocation.into()),
        None => Ok(false),
    }
}

pub(super) fn consume_first_pin_intent(
    transaction: &Transaction<'_>,
    allocation: &StoredRemoteAllocation,
) -> Result<(), Error> {
    if has_first_pin_intent(transaction, allocation, &allocation.worker_request()?)? {
        transaction.execute(
            "DELETE FROM remote_first_pin_intents WHERE workspace_local_id = ?1",
            [&allocation.workspace.state().spec.workspace_local_id],
        )?;
    }
    Ok(())
}
