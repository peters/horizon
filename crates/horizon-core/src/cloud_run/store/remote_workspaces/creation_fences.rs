//! Migration-only, append-only denials: saved runtime identities never grant creation.

use super::super::{CloudJobId, CloudWorkflowId};
use super::{CloudStoreError, MAX_MATERIALIZED_SNAPSHOT_BYTES, WorkspaceRow, decode, validation};
use rusqlite::{Connection, Transaction, params};

const SCHEMA: [&str; 4] = [
    r"CREATE TABLE remote_runtime_creation_fences (
    workflow_id TEXT NOT NULL CHECK (length(workflow_id) = 36),
    job_id TEXT NOT NULL CHECK (length(job_id) = 36),
    PRIMARY KEY (workflow_id, job_id)
) STRICT, WITHOUT ROWID",
    "CREATE INDEX remote_runtime_creation_fences_job ON remote_runtime_creation_fences(job_id)",
    "CREATE TRIGGER remote_runtime_creation_fences_no_update BEFORE UPDATE ON remote_runtime_creation_fences BEGIN SELECT RAISE(ABORT, 'remote creation fences are immutable'); END",
    "CREATE TRIGGER remote_runtime_creation_fences_no_delete BEFORE DELETE ON remote_runtime_creation_fences BEGIN SELECT RAISE(ABORT, 'remote creation fences are immutable'); END",
];

const CLAIM_QUERY: &str = "SELECT
    EXISTS(SELECT 1 FROM remote_runtime_creation_fences WHERE workflow_id = ?1)
    OR EXISTS(SELECT 1 FROM remote_runtime_creation_fences
              INDEXED BY remote_runtime_creation_fences_job WHERE job_id = ?2)";

/// The schema upgrade owns the immediate transaction; any bad snapshot rolls it all back.
/// Stream every session once, materializing at most one size-bounded snapshot at a time.
pub(in crate::cloud_run::store) fn migrate(transaction: &Transaction<'_>) -> Result<(), CloudStoreError> {
    for definition in SCHEMA {
        transaction.execute_batch(definition)?;
    }
    let mut statement = transaction.prepare(
        "SELECT substr(workspace_local_id, 1, 129), substr(session_id, 1, 37), revision,
                substr(snapshot, 1, ?1) FROM remote_workspaces",
    )?;
    let rows = statement.query_map([MAX_MATERIALIZED_SNAPSHOT_BYTES], |row| {
        Ok((row.get::<_, String>(0)?, WorkspaceRow::from_row(row, 1)?))
    })?;
    for row in rows {
        let (workspace_local_id, row) = row?;
        validation::validate_key(&row.session_id, &workspace_local_id)
            .map_err(|_| CloudStoreError::InvalidLegacyRuntimeSnapshot)?;
        let owner = row.session_id.clone();
        let saved =
            decode(&owner, &workspace_local_id, row).map_err(|_| CloudStoreError::InvalidLegacyRuntimeSnapshot)?;
        if let Some(runtime) = saved.state.runtime {
            transaction.execute(
                "INSERT OR IGNORE INTO remote_runtime_creation_fences (workflow_id, job_id) VALUES (?1, ?2)",
                params![runtime.workflow_id.to_string(), runtime.job_id.to_string()],
            )?;
        }
    }
    Ok(())
}

pub(in crate::cloud_run::store) fn validate_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let matches: bool = connection.query_row(
        "SELECT COUNT(*) = 4 AND COUNT(CASE WHEN sql IN (?1, ?2, ?3, ?4) THEN 1 END) = 4
         FROM main.sqlite_schema WHERE tbl_name = 'remote_runtime_creation_fences' AND sql IS NOT NULL",
        SCHEMA,
        |row| row.get(0),
    )?;
    if !matches {
        return Err(CloudStoreError::InvalidCreationFenceSchema);
    }
    Ok(())
}

/// Check inside the claim transaction. These denials confer no ownership or provider authority.
pub(in crate::cloud_run::store) fn ensure_unreferenced(
    connection: &Connection,
    workflow_id: CloudWorkflowId,
    job_id: CloudJobId,
) -> Result<(), CloudStoreError> {
    let fenced: bool = connection.query_row(
        CLAIM_QUERY,
        params![workflow_id.to_string(), job_id.to_string()],
        |row| row.get(0),
    )?;
    if fenced {
        return Err(CloudStoreError::LegacyRuntimeCreationDenied);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
