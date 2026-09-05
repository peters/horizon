//! Session-owned remote records, independent of board saves and provider I/O.
//! These record operations do not authorize lifecycle actions or retire cleanup intent.
//! Operations are synchronous and must run off the render thread.

mod validation;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    CloudStoreError, CloudWorkflowStore, MAX_MATERIALIZED_SNAPSHOT_BYTES, MAX_RECOVERED_SNAPSHOT_BYTES,
    MAX_SNAPSHOT_BYTES, database::ensure_current_schema,
};
use crate::remote_workspace::{RemoteWorkspaceError, RemoteWorkspaceState};
pub(super) use validation::validate_key;
use validation::{validate_replacement, validate_session_id};

const MAX_RECOVERED_WORKSPACES: usize = 512;
const RECOVERY_QUERY: &str = "SELECT substr(workspace_local_id, 1, 129), substr(session_id, 1, 37), revision,
                                   substr(snapshot, 1, ?3)
    FROM remote_workspaces INDEXED BY remote_workspaces_session
    WHERE session_id = ?1 ORDER BY workspace_local_id LIMIT ?2";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRemoteWorkspace {
    session_id: String,
    state: RemoteWorkspaceState,
    revision: u64,
}

impl StoredRemoteWorkspace {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn state(&self) -> &RemoteWorkspaceState {
        &self.state
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl CloudWorkflowStore {
    /// Persist a validated dormant aggregate with one immutable owning session.
    /// A logical workspace identity cannot also belong to a copied session.
    /// Existing legacy runtimes remain readable; new runtime identity requires allocation.
    /// # Errors
    /// Rejects invalid keys/snapshots, existing workspace identities, or storage failures.
    pub fn create_remote_workspace(
        &self,
        session_id: &str,
        state: &RemoteWorkspaceState,
    ) -> Result<StoredRemoteWorkspace, RemoteWorkspaceStoreError> {
        validate_key(session_id, &state.spec.workspace_local_id)?;
        let snapshot = encode(session_id, state)?;
        if state.runtime.is_some() {
            return Err(RemoteWorkspaceStoreError::RuntimeAllocationRequired);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_workspaces WHERE workspace_local_id = ?1)",
            [&state.spec.workspace_local_id],
            |row| row.get(0),
        )?;
        if exists {
            return Err(RemoteWorkspaceStoreError::AlreadyExists);
        }
        super::remote_allocations::validate_workspace_write(&transaction, session_id, None, state)?;
        ensure_session_budget(&transaction, session_id, &state.spec.workspace_local_id, snapshot.len())?;
        transaction.execute(
            "INSERT INTO remote_workspaces (workspace_local_id, session_id, revision, snapshot)
             VALUES (?1, ?2, 1, ?3)",
            params![state.spec.workspace_local_id, session_id, snapshot],
        )?;
        transaction.commit()?;
        Ok(StoredRemoteWorkspace {
            session_id: session_id.into(),
            state: state.clone(),
            revision: 1,
        })
    }

    /// Load one record only for its owning session, revalidating all stored data.
    /// # Errors
    /// Fails closed on ownership mismatch, corruption, incompatible schema, or storage errors.
    pub fn load_remote_workspace(
        &self,
        session_id: &str,
        workspace_local_id: &str,
    ) -> Result<Option<StoredRemoteWorkspace>, RemoteWorkspaceStoreError> {
        validate_key(session_id, workspace_local_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        load_owned(&transaction, session_id, workspace_local_id)
    }

    /// Recover a bounded session-local set in stable workspace identity order.
    /// No partial set is returned if any selected record is invalid or over budget.
    /// # Errors
    /// Rejects invalid sessions, corrupt/oversized records, excessive recovery sets, or storage errors.
    pub fn list_remote_workspaces(
        &self,
        session_id: &str,
    ) -> Result<Vec<StoredRemoteWorkspace>, RemoteWorkspaceStoreError> {
        validate_session_id(session_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let limit = i64::try_from(MAX_RECOVERED_WORKSPACES + 1)
            .map_err(|_| RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
        let mut statement = transaction.prepare(RECOVERY_QUERY)?;
        let rows = statement.query_map(params![session_id, limit, MAX_MATERIALIZED_SNAPSHOT_BYTES], |row| {
            Ok((row.get::<_, String>(0)?, WorkspaceRow::from_row(row, 1)?))
        })?;
        let mut states = Vec::new();
        let mut snapshot_bytes = 0_usize;
        for row in rows {
            let (workspace_local_id, row) = row?;
            snapshot_bytes = snapshot_bytes
                .checked_add(row.snapshot.len())
                .ok_or(RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
            check_recovery_budget(states.len() + 1, snapshot_bytes)?;
            states.push(decode(session_id, &workspace_local_id, row)?);
        }
        Ok(states)
    }

    /// Replace exactly the observed revision and snapshot, never another session's record.
    /// The coordinator remains responsible for lifecycle legality and verified cleanup
    /// before clearing an unbound runtime. This operation cannot introduce a runtime,
    /// rewind a non-creating observation to provisioning, or allocate a workflow or worker.
    /// Bound allocations cannot be cleared here.
    /// # Errors
    /// Rejects stale writers, identity/generation drift, checkpoint loss, invalid snapshots,
    /// revision exhaustion, corruption, or storage failures.
    pub fn replace_remote_workspace(
        &self,
        expected: &StoredRemoteWorkspace,
        next: &RemoteWorkspaceState,
    ) -> Result<StoredRemoteWorkspace, RemoteWorkspaceStoreError> {
        let replacement = WorkspaceReplacement::new(expected, next)?;
        if expected.state.runtime.is_none() && next.runtime.is_some() {
            return Err(RemoteWorkspaceStoreError::RuntimeAllocationRequired);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let stored = replacement.persist(&transaction)?;
        transaction.commit()?;
        Ok(stored)
    }
}

pub(super) struct WorkspaceReplacement<'a> {
    expected: &'a StoredRemoteWorkspace,
    next: &'a RemoteWorkspaceState,
    snapshot: Vec<u8>,
}

impl<'a> WorkspaceReplacement<'a> {
    pub(super) fn new(
        expected: &'a StoredRemoteWorkspace,
        next: &'a RemoteWorkspaceState,
    ) -> Result<Self, RemoteWorkspaceStoreError> {
        validate_key(&expected.session_id, &expected.state.spec.workspace_local_id)?;
        validate_replacement(&expected.state, next)?;
        Ok(Self {
            expected,
            next,
            snapshot: encode(&expected.session_id, next)?,
        })
    }

    /// The caller must check the current schema in an immediate write transaction.
    /// The returned value is provisional until that caller commits. Unlike generic
    /// record writes, the allocator may stage a new runtime here; it must commit
    /// the matching workflow and ownership binding in that same transaction.
    pub(super) fn persist(
        &self,
        transaction: &rusqlite::Transaction<'_>,
    ) -> Result<StoredRemoteWorkspace, RemoteWorkspaceStoreError> {
        let expected = self.expected;
        let next = self.next;
        let current = load_owned(
            transaction,
            &expected.session_id,
            &expected.state.spec.workspace_local_id,
        )?
        .ok_or(RemoteWorkspaceStoreError::Missing)?;
        if current.revision != expected.revision {
            return Err(RemoteWorkspaceStoreError::RevisionConflict {
                expected: expected.revision,
                actual: current.revision,
            });
        }
        if current != *expected {
            return Err(RemoteWorkspaceStoreError::SnapshotConflict);
        }
        super::remote_allocations::validate_workspace_write(
            transaction,
            &expected.session_id,
            Some(&expected.state),
            next,
        )?;
        ensure_session_budget(
            transaction,
            &expected.session_id,
            &expected.state.spec.workspace_local_id,
            self.snapshot.len(),
        )?;
        let revision = expected
            .revision
            .checked_add(1)
            .and_then(|revision| i64::try_from(revision).ok())
            .ok_or(RemoteWorkspaceStoreError::RevisionExhausted)?;
        let changed = transaction.execute(
            "UPDATE remote_workspaces SET revision = ?2, snapshot = ?3
             WHERE workspace_local_id = ?1 AND session_id = ?4",
            params![
                expected.state.spec.workspace_local_id,
                revision,
                self.snapshot,
                expected.session_id
            ],
        )?;
        if changed != 1 {
            return Err(RemoteWorkspaceStoreError::SnapshotConflict);
        }
        Ok(StoredRemoteWorkspace {
            session_id: expected.session_id.clone(),
            state: next.clone(),
            revision: expected.revision + 1,
        })
    }
}

struct WorkspaceRow {
    session_id: String,
    revision: i64,
    snapshot: Vec<u8>,
}

fn ensure_session_budget(
    connection: &Connection,
    session_id: &str,
    replaced_id: &str,
    next_bytes: usize,
) -> Result<(), RemoteWorkspaceStoreError> {
    let limit =
        i64::try_from(MAX_RECOVERED_WORKSPACES + 1).map_err(|_| RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
    let mut statement = connection.prepare(
        "SELECT length(snapshot) FROM remote_workspaces INDEXED BY remote_workspaces_session
         WHERE session_id = ?1 AND workspace_local_id != ?2 LIMIT ?3",
    )?;
    let sizes = statement.query_map(params![session_id, replaced_id, limit], |row| row.get::<_, i64>(0))?;
    let mut count = 1;
    let mut bytes = next_bytes;
    check_recovery_budget(count, bytes)?;
    for size in sizes {
        let size = usize::try_from(size?).map_err(|_| RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
        bytes = bytes
            .checked_add(size)
            .ok_or(RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
        count += 1;
        check_recovery_budget(count, bytes)?;
    }
    Ok(())
}

impl WorkspaceRow {
    fn from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            session_id: row.get(offset)?,
            revision: row.get(offset + 1)?,
            snapshot: row.get(offset + 2)?,
        })
    }
}

pub(super) fn load_owned(
    connection: &Connection,
    session_id: &str,
    workspace_local_id: &str,
) -> Result<Option<StoredRemoteWorkspace>, RemoteWorkspaceStoreError> {
    connection
        .query_row(
            "SELECT substr(session_id, 1, 37), revision, substr(snapshot, 1, ?2)
             FROM remote_workspaces WHERE workspace_local_id = ?1",
            params![workspace_local_id, MAX_MATERIALIZED_SNAPSHOT_BYTES],
            |row| WorkspaceRow::from_row(row, 0),
        )
        .optional()?
        .map(|row| decode(session_id, workspace_local_id, row))
        .transpose()
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceSnapshot<T> {
    session_id: String,
    state: T,
}

fn encode(session_id: &str, state: &RemoteWorkspaceState) -> Result<Vec<u8>, RemoteWorkspaceStoreError> {
    state.validate()?;
    let mut buffer = SnapshotBuffer::default();
    serde_json::to_writer(
        &mut buffer,
        &WorkspaceSnapshot {
            session_id: session_id.into(),
            state,
        },
    )
    .map_err(|_| {
        if buffer.too_large {
            RemoteWorkspaceStoreError::SnapshotTooLarge
        } else {
            RemoteWorkspaceStoreError::InvalidStoredSnapshot
        }
    })?;
    Ok(buffer.bytes)
}

#[derive(Default)]
struct SnapshotBuffer {
    bytes: Vec<u8>,
    too_large: bool,
}

impl std::io::Write for SnapshotBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_SNAPSHOT_BYTES - self.bytes.len() {
            self.too_large = true;
            return Err(std::io::Error::other("remote snapshot size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn decode(
    session_id: &str,
    workspace_local_id: &str,
    row: WorkspaceRow,
) -> Result<StoredRemoteWorkspace, RemoteWorkspaceStoreError> {
    if row.session_id != session_id {
        return Err(RemoteWorkspaceStoreError::OwnershipMismatch);
    }
    ensure_snapshot_size(row.snapshot.len())?;
    // The aggregate's deserializer validates its schema, identities, and all domain invariants.
    let snapshot: WorkspaceSnapshot<RemoteWorkspaceState> =
        serde_json::from_slice(&row.snapshot).map_err(|_| RemoteWorkspaceStoreError::InvalidStoredSnapshot)?;
    let revision = u64::try_from(row.revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or(RemoteWorkspaceStoreError::InvalidStoredSnapshot)?;
    if snapshot.session_id != session_id || snapshot.state.spec.workspace_local_id != workspace_local_id {
        return Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot);
    }
    Ok(StoredRemoteWorkspace {
        session_id: row.session_id,
        state: snapshot.state,
        revision,
    })
}

fn ensure_snapshot_size(size: usize) -> Result<(), RemoteWorkspaceStoreError> {
    if size > MAX_SNAPSHOT_BYTES {
        return Err(RemoteWorkspaceStoreError::SnapshotTooLarge);
    }
    Ok(())
}

fn check_recovery_budget(count: usize, bytes: usize) -> Result<(), RemoteWorkspaceStoreError> {
    if count > MAX_RECOVERED_WORKSPACES || bytes > MAX_RECOVERED_SNAPSHOT_BYTES {
        return Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded);
    }
    Ok(())
}

/// Errors omit snapshot content, command arguments, task handoffs, and remote endpoint values.
#[derive(Debug, Error)]
pub enum RemoteWorkspaceStoreError {
    #[error(transparent)]
    Workspace(#[from] RemoteWorkspaceError),
    #[error(transparent)]
    Storage(#[from] CloudStoreError),
    #[error("remote workspace database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("remote workspace owner must be a canonical session UUID")]
    InvalidSessionId,
    #[error("invalid remote workspace identity")]
    InvalidWorkspaceId,
    #[error("remote workspace identity already exists")]
    AlreadyExists,
    #[error("remote workspace record is missing")]
    Missing,
    #[error("remote workspace belongs to a different session")]
    OwnershipMismatch,
    #[error("remote workspace revision changed from expected {expected} to {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("remote workspace snapshot does not match its revision")]
    SnapshotConflict,
    #[error("remote workspace exhausted its revision range")]
    RevisionExhausted,
    #[error("stored remote workspace snapshot is invalid")]
    InvalidStoredSnapshot,
    #[error("remote workspace snapshot exceeds the storage size limit")]
    SnapshotTooLarge,
    #[error("remote workspace recovery exceeds the bounded storage budget")]
    RecoveryLimitExceeded,
    #[error("replacement changes remote workspace or active runtime identity")]
    ReplacementIdentityMismatch,
    #[error(
        "replacement loses or rewinds a remote generation, observed phase, cleanup intent, or repository checkpoint"
    )]
    NonMonotonicReplacement,
    #[error("new remote runtime identity requires atomic allocation")]
    RuntimeAllocationRequired,
    #[error("remote workspace already has a runtime to reconcile")]
    RuntimeAlreadyActive,
    #[error("remote workspace exhausted its allocation generation range")]
    GenerationExhausted,
    #[error("remote allocation setup retention must end after its valid creation timestamp")]
    InvalidAllocationRetention,
    #[error("active remote snapshot has no verified workflow allocation; reconciliation is required")]
    UnboundRuntime,
}

#[cfg(test)]
mod tests;
