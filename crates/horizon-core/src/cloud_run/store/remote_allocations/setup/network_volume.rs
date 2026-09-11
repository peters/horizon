//! Generation-owned selection intent. No provider verification, adoption or creation authority.

use super::{
    CloudStoreError, CloudWorkflowStore, Error, StoredRemoteAllocation, ensure_current_schema, exact_allocation,
    open_read_connection, validate_unclaimed,
};
use crate::cloud_run::{CloudProvider, WorkerLifetime, runpod::RunPodNetworkVolumeExpectation};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

impl CloudWorkflowStore {
    /// Record one immutable HPS volume selection for this exact allocation.
    ///
    /// This is inert, non-secret intent: it neither verifies provider storage nor
    /// establishes ownership, exclusivity, attachment or provisioning authority.
    /// It does not bump the allocation revision or consume/renew a creation grant.
    /// Explicit task-free setup and its retry/recovery bind the provider to this selection.
    /// No retirement or next-generation rebinding is exposed for this workspace,
    /// matching the existing allocation and first-pin boundaries.
    ///
    /// First recording must precede first-pin intent and worker creation. An exact
    /// repeat is a read even after setup closes; a different selection never replaces
    /// the row. Run this synchronous operation off the render thread.
    /// # Errors
    /// Rejects stale/foreign allocations, invalid selections, unsupported targets,
    /// late first recording, corrupt storage and schema or database failures.
    pub fn record_remote_network_volume_selection(
        &self,
        expected: &StoredRemoteAllocation,
        selection: &RunPodNetworkVolumeExpectation,
    ) -> Result<(), Error> {
        if !valid_selection(selection) {
            return Err(Error::InvalidNetworkVolumeSelection);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        validate_target(&current)?;
        if let Some(existing) = load_selection(&transaction, &current)? {
            return if existing == *selection {
                Ok(())
            } else {
                Err(Error::ReplacementIdentityMismatch)
            };
        }
        validate_unclaimed(&transaction, &current)?;
        let state = current.workspace().state();
        let has_first_pin: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_first_pin_intents WHERE workspace_local_id = ?1)",
            [&state.spec.workspace_local_id],
            |row| row.get(0),
        )?;
        if has_first_pin {
            return Err(Error::RuntimeSetupUnavailable);
        }
        let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        transaction.execute(
            "INSERT INTO remote_network_volume_selections
             (workspace_local_id, session_id, generation, workflow_id, job_id, version,
              volume_id, data_center_id, minimum_size_gb, storage_type)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, 'HIGH_PERFORMANCE')",
            params![
                state.spec.workspace_local_id,
                current.workspace().session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string(),
                selection.volume_id,
                selection.data_center_id,
                selection.minimum_size_gb
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Load inert selection intent from one exact, consistent allocation snapshot.
    /// No writes, database creation/migration, provider calls or authority renewal.
    /// Valid schema-four/five stores return `None`; migration never backfills rows.
    /// An existing selection remains readable after setup expiry or management intent.
    /// # Errors
    /// Rejects stale/foreign allocations, malformed or mismatched rows and storage errors.
    pub fn load_remote_network_volume_selection(
        &self,
        expected: &StoredRemoteAllocation,
    ) -> Result<Option<RunPodNetworkVolumeExpectation>, Error> {
        let mut connection = open_read_connection(self.path())?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        let version = transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if matches!(version, 4 | 5) {
            return Ok(None);
        }
        load_selection(&transaction, &current)
    }
}

fn validate_target(allocation: &StoredRemoteAllocation) -> Result<(), Error> {
    let target = &allocation.workspace().state().spec.target;
    if target.provider != CloudProvider::RunPod || target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::RuntimeSetupUnavailable);
    }
    Ok(())
}

fn valid_selection(selection: &RunPodNetworkVolumeExpectation) -> bool {
    // Frozen version-one storage bounds; future provider changes do not reinterpret saved rows.
    let valid_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 191
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    };
    valid_id(&selection.volume_id)
        && valid_id(&selection.data_center_id)
        && (10..=4_096).contains(&selection.minimum_size_gb)
}

fn load_selection(
    connection: &Connection,
    allocation: &StoredRemoteAllocation,
) -> Result<Option<RunPodNetworkVolumeExpectation>, Error> {
    let state = allocation.workspace().state();
    let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
    let row = connection
        .query_row(
        "SELECT workspace_local_id = ?1 AND session_id = ?2 AND generation = ?3 AND workflow_id = ?4 AND job_id = ?5
                AND version = 1 AND storage_type = 'HIGH_PERFORMANCE'
                AND (SELECT COUNT(*) FROM remote_network_volume_selections
                     WHERE workspace_local_id = ?1 OR workflow_id = ?4 OR job_id = ?5) = 1,
                CAST(substr(CAST(volume_id AS BLOB), 1, 192) AS TEXT),
                CAST(substr(CAST(data_center_id AS BLOB), 1, 192) AS TEXT), minimum_size_gb
         FROM remote_network_volume_selections
         WHERE workspace_local_id = ?1 OR workflow_id = ?4 OR job_id = ?5",
            params![
                state.spec.workspace_local_id,
                allocation.workspace().session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    RunPodNetworkVolumeExpectation {
                        volume_id: row.get(1)?,
                        data_center_id: row.get(2)?,
                        minimum_size_gb: row.get(3)?,
                    },
                ))
            },
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((true, selection)) if valid_selection(&selection) && validate_target(allocation).is_ok() => {
            Ok(Some(selection))
        }
        Some(_) => Err(CloudStoreError::InvalidRemoteAllocation.into()),
    }
}

#[cfg(test)]
mod tests;
