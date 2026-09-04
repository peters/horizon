//! Durable local control-plane storage for cloud workflow snapshots and creation claims.

use std::collections::HashMap;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use super::{
    CloudJobId, CloudJobOutcome, CloudJobState, CloudProtocolError, CloudProvider, CloudWorkflow, CloudWorkflowId,
    WorkerTarget,
};
use crate::HorizonHome;

const STORE_SCHEMA_VERSION: i64 = 1;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const MAX_MATERIALIZED_SNAPSHOT_BYTES: i64 = 4 * 1024 * 1024 + 1;
const MAX_RECOVERED_WORKFLOWS: usize = 512;
const MAX_RECOVERED_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS cloud_workflows (
    workflow_id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at_millis INTEGER NOT NULL,
    updated_at_millis INTEGER NOT NULL,
    retain_until_millis INTEGER NOT NULL,
    snapshot BLOB NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS cloud_workflows_retention
    ON cloud_workflows(retain_until_millis, updated_at_millis);
CREATE TABLE IF NOT EXISTS cloud_worker_creation_claims (
    provider TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    resource_name TEXT NOT NULL,
    claimed_at_millis INTEGER NOT NULL,
    PRIMARY KEY (provider, resource_name),
    UNIQUE (provider, workflow_id, job_id),
    FOREIGN KEY (workflow_id) REFERENCES cloud_workflows(workflow_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX IF NOT EXISTS cloud_worker_creation_claims_workflow
    ON cloud_worker_creation_claims(workflow_id, job_id, provider);
";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredWorkflow {
    workflow: CloudWorkflow,
    revision: u64,
}

impl StoredWorkflow {
    #[must_use]
    pub fn workflow(&self) -> &CloudWorkflow {
        &self.workflow
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn into_workflow(self) -> CloudWorkflow {
        self.workflow
    }
}

/// SQLite-backed workflow storage. Connections are short-lived so clones are
/// safe to use from separate controller threads and processes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudWorkflowStore {
    path: PathBuf,
}

impl CloudWorkflowStore {
    /// Open the default store below the supplied Horizon home.
    ///
    /// # Errors
    /// Fails when the private store cannot be created or its schema is newer
    /// than this binary understands.
    pub fn open(home: &HorizonHome) -> Result<Self, CloudStoreError> {
        Self::open_path(home.cloud_workflow_store_path())
    }

    /// Open an explicit store path, primarily for isolated hosts and tests.
    ///
    /// # Errors
    /// Fails when the private store cannot be created or initialized.
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self, CloudStoreError> {
        let path = path.into();
        let store = Self {
            path: prepare_private_store(&path)?,
        };
        let mut connection = store.connection()?;
        initialize_schema(&mut connection)?;
        Ok(store)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert the first valid snapshot for a workflow at revision one.
    ///
    /// # Errors
    /// Rejects invalid snapshots, duplicate workflow identities, and storage
    /// failures.
    pub fn create(&self, workflow: &CloudWorkflow) -> Result<StoredWorkflow, CloudStoreError> {
        let snapshot = encode_workflow(workflow)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let id = workflow.id.to_string();
        if workflow_row(&transaction, &id)?.is_some() {
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
                snapshot
            ],
        )?;
        transaction.commit()?;
        Ok(StoredWorkflow {
            workflow: workflow.clone(),
            revision: 1,
        })
    }

    /// Load and revalidate one workflow snapshot.
    ///
    /// # Errors
    /// Fails closed when the stored snapshot or indexed metadata is corrupt.
    pub fn load(&self, workflow_id: CloudWorkflowId) -> Result<Option<StoredWorkflow>, CloudStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        workflow_row(&transaction, &workflow_id.to_string())?
            .map(|row| decode_workflow_row(workflow_id, &row))
            .transpose()
    }

    /// List non-expired workflow snapshots for restart recovery.
    ///
    /// # Errors
    /// Fails closed if any selected snapshot is corrupt or the retained set
    /// exceeds the bounded recovery budget.
    pub fn list_retained(&self, now_millis: i64) -> Result<Vec<StoredWorkflow>, CloudStoreError> {
        if now_millis < 0 {
            return Err(CloudStoreError::InvalidTimestamp);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let query_limit =
            isize::try_from(MAX_RECOVERED_WORKFLOWS + 1).map_err(|_| CloudStoreError::RecoveryLimitExceeded)?;
        let mut statement = transaction.prepare(
            "SELECT substr(workflow_id, 1, 37), revision, created_at_millis, updated_at_millis, retain_until_millis,
                    substr(snapshot, 1, ?3)
             FROM cloud_workflows
             INDEXED BY cloud_workflows_retention
             WHERE retain_until_millis >= ?1
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![now_millis, query_limit, MAX_MATERIALIZED_SNAPSHOT_BYTES],
            |row| Ok((row.get::<_, String>(0)?, WorkflowRow::from_row(row, 1)?)),
        )?;
        let mut retained_rows = Vec::new();
        let mut snapshot_bytes = 0_usize;
        for row in rows {
            let (id, row) = row?;
            snapshot_bytes = snapshot_bytes
                .checked_add(row.snapshot.len())
                .ok_or(CloudStoreError::RecoveryLimitExceeded)?;
            check_recovery_budget(retained_rows.len() + 1, snapshot_bytes)?;
            let workflow_id = parse_workflow_id(&id)?;
            retained_rows.push((id, workflow_id, row));
        }
        retained_rows.sort_unstable_by(|left, right| {
            right
                .2
                .updated_at_millis
                .cmp(&left.2.updated_at_millis)
                .then_with(|| left.0.cmp(&right.0))
        });
        retained_rows
            .into_iter()
            .map(|(_, workflow_id, row)| decode_workflow_row(workflow_id, &row))
            .collect()
    }

    /// Replace a snapshot only when the caller still owns its exact revision.
    ///
    /// # Errors
    /// Rejects stale writers, identity changes, shortened retention, invalid
    /// snapshots, and storage failures.
    pub fn replace(&self, expected: &StoredWorkflow, next: &CloudWorkflow) -> Result<StoredWorkflow, CloudStoreError> {
        validate_replacement(expected.workflow(), next)?;
        let snapshot = encode_workflow(next)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let id = expected.workflow.id.to_string();
        let current = workflow_row(&transaction, &id)?
            .ok_or(CloudStoreError::WorkflowMissing(expected.workflow.id))
            .and_then(|row| decode_workflow_row(expected.workflow.id, &row))?;
        if current.revision != expected.revision {
            return Err(CloudStoreError::RevisionConflict {
                expected: expected.revision,
                actual: current.revision,
            });
        }
        if current.workflow != expected.workflow {
            return Err(CloudStoreError::SnapshotConflict(expected.workflow.id));
        }
        ensure_claimed_targets_unchanged(&transaction, &id, &current.workflow, next)?;
        let revision = expected
            .revision
            .checked_add(1)
            .ok_or(CloudStoreError::RevisionExhausted(expected.workflow.id))?;
        let stored_revision =
            i64::try_from(revision).map_err(|_| CloudStoreError::RevisionExhausted(expected.workflow.id))?;
        transaction.execute(
            "UPDATE cloud_workflows
             SET revision = ?2, updated_at_millis = ?3, retain_until_millis = ?4, snapshot = ?5
             WHERE workflow_id = ?1",
            params![
                id,
                stored_revision,
                next.updated_at_millis,
                next.retain_until_millis,
                snapshot
            ],
        )?;
        transaction.commit()?;
        Ok(StoredWorkflow {
            workflow: next.clone(),
            revision,
        })
    }

    /// Atomically record the one provider create permission for a worker job.
    /// The claim is linked to a persisted workflow and survives process exit.
    ///
    /// # Errors
    /// Rejects missing/mismatched jobs, malformed resource names, conflicting
    /// claim identities, corrupt snapshots, and storage failures.
    pub fn claim_worker_creation(
        &self,
        workflow_id: CloudWorkflowId,
        job_id: CloudJobId,
        target: &WorkerTarget,
        resource_name: &str,
    ) -> Result<bool, CloudStoreError> {
        if !valid_resource_name(resource_name) {
            return Err(CloudStoreError::InvalidResourceName);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let id = workflow_id.to_string();
        let workflow = workflow_row(&transaction, &id)?
            .ok_or(CloudStoreError::WorkflowMissing(workflow_id))
            .and_then(|row| decode_workflow_row(workflow_id, &row))?;
        let claimed_at_millis = current_unix_millis()?;
        if workflow.workflow.retain_until_millis < claimed_at_millis {
            return Err(CloudStoreError::WorkflowExpired(workflow_id));
        }
        validate_claim_target(&workflow.workflow, job_id, target)?;
        let provider_name = provider_name(target.provider);
        let job_id_text = job_id.to_string();
        let existing = transaction
            .query_row(
                "SELECT substr(workflow_id, 1, 37), substr(job_id, 1, 37)
                 FROM cloud_worker_creation_claims
                 WHERE provider = ?1 AND resource_name = ?2",
                params![provider_name, resource_name],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let resource_matches = match existing {
            Some(existing) if existing.0 == id && existing.1 == job_id_text => true,
            Some(_) => return Err(CloudStoreError::ClaimIdentityConflict),
            None => false,
        };
        let job_claimed_elsewhere = transaction.query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM cloud_worker_creation_claims
                    WHERE workflow_id = ?1 AND job_id = ?2
                      AND (provider != ?3 OR resource_name != ?4)
                 )",
            params![id, job_id_text, provider_name, resource_name],
            |row| row.get::<_, bool>(0),
        )?;
        if job_claimed_elsewhere {
            return Err(CloudStoreError::ClaimIdentityConflict);
        }
        if resource_matches {
            transaction.commit()?;
            return Ok(false);
        }
        transaction.execute(
            "INSERT INTO cloud_worker_creation_claims (
                provider, workflow_id, job_id, resource_name, claimed_at_millis
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![provider_name, id, job_id_text, resource_name, claimed_at_millis],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    fn connection(&self) -> Result<Connection, CloudStoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&self.path, flags)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(connection)
    }
}

struct WorkflowRow {
    revision: i64,
    created_at_millis: i64,
    updated_at_millis: i64,
    retain_until_millis: i64,
    snapshot: Vec<u8>,
}

impl WorkflowRow {
    fn from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            revision: row.get(offset)?,
            created_at_millis: row.get(offset + 1)?,
            updated_at_millis: row.get(offset + 2)?,
            retain_until_millis: row.get(offset + 3)?,
            snapshot: row.get(offset + 4)?,
        })
    }
}

fn initialize_schema(connection: &mut Connection) -> Result<(), CloudStoreError> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if version != 0 && version != STORE_SCHEMA_VERSION {
        return Err(CloudStoreError::UnsupportedSchema(version));
    }
    transaction.execute_batch(SCHEMA)?;
    transaction.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn ensure_current_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let version = connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if version != STORE_SCHEMA_VERSION {
        return Err(CloudStoreError::UnsupportedSchema(version));
    }
    Ok(())
}

fn workflow_row(connection: &Connection, workflow_id: &str) -> rusqlite::Result<Option<WorkflowRow>> {
    connection
        .query_row(
            "SELECT revision, created_at_millis, updated_at_millis, retain_until_millis, substr(snapshot, 1, ?2)
             FROM cloud_workflows WHERE workflow_id = ?1",
            params![workflow_id, MAX_MATERIALIZED_SNAPSHOT_BYTES],
            |row| WorkflowRow::from_row(row, 0),
        )
        .optional()
}

fn ensure_claimed_targets_unchanged(
    connection: &Connection,
    workflow_id: &str,
    current: &CloudWorkflow,
    next: &CloudWorkflow,
) -> Result<(), CloudStoreError> {
    let current_targets: HashMap<_, _> = current
        .nodes
        .iter()
        .filter_map(|node| node.worker.as_ref().map(|target| (node.id, target)))
        .collect();
    let next_targets: HashMap<_, _> = next
        .nodes
        .iter()
        .filter_map(|node| node.worker.as_ref().map(|target| (node.id, target)))
        .collect();
    let query_limit = current_targets
        .len()
        .checked_add(1)
        .and_then(|limit| i64::try_from(limit).ok())
        .ok_or(CloudStoreError::InvalidStoredCreationClaim)?;
    let mut statement = connection.prepare(
        "SELECT substr(provider, 1, 9), substr(job_id, 1, 37)
         FROM cloud_worker_creation_claims
         WHERE workflow_id = ?1
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![workflow_id, query_limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for (index, row) in rows.enumerate() {
        if index >= current_targets.len() {
            return Err(CloudStoreError::InvalidStoredCreationClaim);
        }
        let (provider, job_id) = row?;
        let job_id = parse_job_id(&job_id)?;
        let target = current_targets
            .get(&job_id)
            .copied()
            .filter(|target| provider == provider_name(target.provider))
            .ok_or(CloudStoreError::InvalidStoredCreationClaim)?;
        if next_targets.get(&job_id).copied() != Some(target) {
            return Err(CloudStoreError::ClaimedTargetChanged(job_id));
        }
    }
    Ok(())
}

fn encode_workflow(workflow: &CloudWorkflow) -> Result<Vec<u8>, CloudStoreError> {
    workflow.validate()?;
    let snapshot = serde_json::to_vec(workflow)?;
    ensure_snapshot_size(snapshot.len())?;
    Ok(snapshot)
}

fn decode_workflow_row(expected_id: CloudWorkflowId, row: &WorkflowRow) -> Result<StoredWorkflow, CloudStoreError> {
    ensure_snapshot_size(row.snapshot.len())?;
    let workflow: CloudWorkflow =
        serde_json::from_slice(&row.snapshot).map_err(|error| CloudStoreError::InvalidStoredWorkflow {
            workflow_id: expected_id,
            reason: error.to_string(),
        })?;
    workflow
        .validate()
        .map_err(|error| CloudStoreError::InvalidStoredWorkflow {
            workflow_id: expected_id,
            reason: error.to_string(),
        })?;
    let revision = u64::try_from(row.revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or(CloudStoreError::InvalidStoredWorkflow {
            workflow_id: expected_id,
            reason: "revision must be positive".to_string(),
        })?;
    if workflow.id != expected_id
        || workflow.created_at_millis != row.created_at_millis
        || workflow.updated_at_millis != row.updated_at_millis
        || workflow.retain_until_millis != row.retain_until_millis
    {
        return Err(CloudStoreError::InvalidStoredWorkflow {
            workflow_id: expected_id,
            reason: "indexed metadata does not match the snapshot".to_string(),
        });
    }
    Ok(StoredWorkflow { workflow, revision })
}

fn validate_replacement(previous: &CloudWorkflow, next: &CloudWorkflow) -> Result<(), CloudStoreError> {
    if previous.id != next.id
        || previous.protocol_version != next.protocol_version
        || previous.created_at_millis != next.created_at_millis
    {
        return Err(CloudStoreError::ReplacementIdentityMismatch);
    }
    if next.updated_at_millis <= previous.updated_at_millis || next.retain_until_millis < previous.retain_until_millis {
        return Err(CloudStoreError::NonMonotonicReplacement);
    }
    Ok(())
}

fn validate_claim_target(
    workflow: &CloudWorkflow,
    job_id: CloudJobId,
    target: &WorkerTarget,
) -> Result<(), CloudStoreError> {
    let nodes: HashMap<_, _> = workflow.nodes.iter().map(|node| (node.id, node)).collect();
    let node = nodes
        .get(&job_id)
        .copied()
        .filter(|node| node.worker.as_ref() == Some(target))
        .ok_or(CloudStoreError::ClaimTargetMismatch(job_id))?;
    let active = matches!(node.state, CloudJobState::Queued | CloudJobState::Provisioning);
    let dependencies_ready = node.depends_on.iter().all(|dependency_id| {
        nodes.get(dependency_id).is_some_and(|dependency| {
            matches!(dependency.state, CloudJobState::Completed | CloudJobState::Cleaned)
                && dependency.outcome == Some(CloudJobOutcome::Succeeded)
        })
    });
    if !active || !dependencies_ready {
        return Err(CloudStoreError::ClaimTargetNotReady(job_id));
    }
    Ok(())
}

fn ensure_snapshot_size(size: usize) -> Result<(), CloudStoreError> {
    if size > MAX_SNAPSHOT_BYTES {
        return Err(CloudStoreError::SnapshotTooLarge {
            size,
            maximum: MAX_SNAPSHOT_BYTES,
        });
    }
    Ok(())
}

fn check_recovery_budget(workflow_count: usize, snapshot_bytes: usize) -> Result<(), CloudStoreError> {
    if workflow_count > MAX_RECOVERED_WORKFLOWS || snapshot_bytes > MAX_RECOVERED_SNAPSHOT_BYTES {
        return Err(CloudStoreError::RecoveryLimitExceeded);
    }
    Ok(())
}

fn parse_workflow_id(value: &str) -> Result<CloudWorkflowId, CloudStoreError> {
    let id: CloudWorkflowId = value.parse().map_err(|_| CloudStoreError::InvalidStoredWorkflowId)?;
    (id.to_string() == value)
        .then_some(id)
        .ok_or(CloudStoreError::InvalidStoredWorkflowId)
}

fn parse_job_id(value: &str) -> Result<CloudJobId, CloudStoreError> {
    let id: CloudJobId = value.parse().map_err(|_| CloudStoreError::InvalidStoredCreationClaim)?;
    (id.to_string() == value)
        .then_some(id)
        .ok_or(CloudStoreError::InvalidStoredCreationClaim)
}

fn valid_resource_name(value: &str) -> bool {
    (1..=191).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn provider_name(provider: CloudProvider) -> &'static str {
    match provider {
        CloudProvider::Azure => "azure",
        CloudProvider::RunPod => "run_pod",
        CloudProvider::LocalDocker => "local_docker",
    }
}

fn current_unix_millis() -> Result<i64, CloudStoreError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CloudStoreError::InvalidTimestamp)?
        .as_millis();
    i64::try_from(millis).map_err(|_| CloudStoreError::InvalidTimestamp)
}

fn prepare_private_store(path: &Path) -> Result<PathBuf, CloudStoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(parent)?;
        if parent.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(CloudStoreError::InsecureStoreDirectory);
        }
    }
    #[cfg(unix)]
    let path = parent
        .canonicalize()?
        .join(path.file_name().unwrap_or(path.as_os_str()));
    #[cfg(not(unix))]
    let path = path.to_path_buf();
    #[cfg(not(unix))]
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(CloudStoreError::SymlinkStorePath),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        let mut options = OpenOptions::new();
        options
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(CloudStoreError::SymlinkStorePath),
            Err(error) => return Err(error.into()),
        };
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

#[derive(Debug, Error)]
pub enum CloudStoreError {
    #[error(transparent)]
    Protocol(#[from] CloudProtocolError),
    #[error("cloud workflow JSON could not be encoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cloud workflow store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cloud workflow store directory must not be accessible by group or other users")]
    InsecureStoreDirectory,
    #[error("cloud workflow store path must not be a symbolic link")]
    SymlinkStorePath,
    #[error("cloud workflow store database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("cloud workflow store schema {0} is not supported by this binary")]
    UnsupportedSchema(i64),
    #[error("cloud workflow {0} already exists")]
    WorkflowExists(CloudWorkflowId),
    #[error("cloud workflow {0} does not exist")]
    WorkflowMissing(CloudWorkflowId),
    #[error("cloud workflow {0} is past its retention deadline")]
    WorkflowExpired(CloudWorkflowId),
    #[error("cloud workflow revision changed from expected {expected} to {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("cloud workflow {0} snapshot does not match its revision")]
    SnapshotConflict(CloudWorkflowId),
    #[error("cloud workflow {0} exhausted its revision range")]
    RevisionExhausted(CloudWorkflowId),
    #[error("replacement changes immutable cloud workflow identity")]
    ReplacementIdentityMismatch,
    #[error("replacement cloud workflow timestamps or retention are not monotonic")]
    NonMonotonicReplacement,
    #[error("stored cloud workflow {workflow_id} is invalid: {reason}")]
    InvalidStoredWorkflow {
        workflow_id: CloudWorkflowId,
        reason: String,
    },
    #[error("stored cloud workflow has an invalid identity")]
    InvalidStoredWorkflowId,
    #[error("cloud workflow snapshot has at least {size} bytes; maximum is {maximum}")]
    SnapshotTooLarge { size: usize, maximum: usize },
    #[error("retained cloud workflows exceed the bounded recovery budget")]
    RecoveryLimitExceeded,
    #[error("cloud worker resource name is invalid")]
    InvalidResourceName,
    #[error("requested worker target does not exactly match persisted cloud job {0}")]
    ClaimTargetMismatch(CloudJobId),
    #[error("cloud job {0} is not ready for worker creation")]
    ClaimTargetNotReady(CloudJobId),
    #[error("cloud job {0} cannot change its worker target after creation was claimed")]
    ClaimedTargetChanged(CloudJobId),
    #[error("stored cloud worker creation claim is invalid")]
    InvalidStoredCreationClaim,
    #[error("cloud worker creation claim conflicts with a different persisted identity")]
    ClaimIdentityConflict,
    #[error("cloud workflow timestamp is invalid")]
    InvalidTimestamp,
}

#[cfg(test)]
mod tests;
