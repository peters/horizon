//! Bounded remote-environment inventory independent of local session files.

use rusqlite::params;

use super::{RemoteWorkspaceStoreError, StoredRemoteWorkspace, WorkspaceRow, decode, validate_key};
use crate::cloud_run::store::{
    CloudWorkflowStore, MAX_MATERIALIZED_SNAPSHOT_BYTES, MAX_RECOVERED_SNAPSHOT_BYTES, MAX_SNAPSHOT_BYTES,
    database::ensure_current_schema,
};
use crate::remote_workspace::valid_local_id;

const PAGE_SIZE: usize = MAX_RECOVERED_SNAPSHOT_BYTES / MAX_SNAPSHOT_BYTES;
const PAGE_QUERY: &str = "SELECT CAST(substr(CAST(workspace_local_id AS BLOB), 1, 129) AS TEXT),
                               CAST(substr(CAST(session_id AS BLOB), 1, 37) AS TEXT), revision,
                               substr(snapshot, 1, ?4)
    FROM remote_workspaces WHERE workspace_local_id >= ?1 AND (?2 IS NULL OR workspace_local_id != ?2)
    ORDER BY workspace_local_id LIMIT ?3";

/// Saved records, not fresh provider observations or permission to manage compute.
/// Each page is a consistent database read; separate pages may observe later writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEnvironmentPage {
    pub records: Vec<StoredRemoteWorkspace>,
    /// Exclusive workspace identity cursor. None means no later record existed at this read.
    /// Restart from the first page to see insertions at or before a previous cursor.
    pub next_cursor: Option<String>,
}

impl CloudWorkflowStore {
    /// List at most 16 saved remote environments across every owning session.
    /// Does not consult client snapshots, filter runtime state/expiry, or call providers.
    /// Run off the render thread and revalidate ownership and provider identity before actions.
    ///
    /// # Errors
    /// Rejects invalid cursors, corrupt/oversized selected records, incompatible schema,
    /// or storage failures. Never treats a failed read as an empty or partial page.
    pub fn list_remote_environment_page(
        &self,
        after_workspace_id: Option<&str>,
    ) -> Result<RemoteEnvironmentPage, RemoteWorkspaceStoreError> {
        if after_workspace_id.is_some_and(|id| !valid_local_id(id)) {
            return Err(RemoteWorkspaceStoreError::InvalidWorkspaceId);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let limit = i64::try_from(PAGE_SIZE).map_err(|_| RemoteWorkspaceStoreError::RecoveryLimitExceeded)?;
        let mut statement = transaction.prepare(PAGE_QUERY)?;
        let rows = statement.query_map(
            params![
                after_workspace_id.unwrap_or_default(),
                after_workspace_id,
                limit,
                MAX_MATERIALIZED_SNAPSHOT_BYTES
            ],
            |row| Ok((row.get::<_, String>(0)?, WorkspaceRow::from_row(row, 1)?)),
        )?;
        let mut records = Vec::with_capacity(PAGE_SIZE);
        for row in rows {
            let (workspace_local_id, row) = row?;
            validate_key(&row.session_id, &workspace_local_id)?;
            let owner = row.session_id.clone();
            records.push(decode(&owner, &workspace_local_id, row)?);
        }
        let mut next_cursor = None;
        if let Some(last) = records.last() {
            let last_id = &last.state().spec.workspace_local_id;
            // Probe only the index, never materialize an extra potentially oversized snapshot.
            let has_more: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM remote_workspaces WHERE workspace_local_id > ?1)",
                [last_id],
                |row| row.get(0),
            )?;
            if has_more {
                next_cursor = Some(last_id.clone());
            }
        }
        Ok(RemoteEnvironmentPage { records, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_uses_a_bounded_primary_key_range_without_sorting() {
        let directory = tempfile::tempdir().expect("private fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control-plane/store.sqlite3")).expect("store");
        let connection = store.connection().expect("connection");
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {PAGE_QUERY}"))
            .expect("query plan");
        let plan: Vec<String> = statement
            .query_map(
                params!["workspace-0016", "workspace-0016", 16, MAX_MATERIALIZED_SNAPSHOT_BYTES],
                |row| row.get(3),
            )
            .expect("plan rows")
            .collect::<Result<_, _>>()
            .expect("plan details");
        assert!(
            plan.iter()
                .any(|detail| detail.contains("SEARCH remote_workspaces USING INDEX"))
        );
        assert!(
            plan.iter()
                .all(|detail| !detail.contains("TEMP B-TREE") && !detail.contains("SCAN"))
        );
        assert_eq!(PAGE_SIZE, 16);
    }
}
