//! Private database opening and schema boundary shared by control-plane records.

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use super::{CloudStoreError, remote_workspaces::creation_fences};

const STORE_SCHEMA_VERSION: i64 = 7;
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

const REMOTE_WORKSPACE_SCHEMA: &str = r"
CREATE TABLE remote_workspaces (
    workspace_local_id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    snapshot BLOB NOT NULL
) STRICT;
CREATE INDEX remote_workspaces_session
    ON remote_workspaces(session_id, workspace_local_id);
";

const REMOTE_ALLOCATION_SCHEMA: [&str; 3] = [
    r"CREATE TABLE remote_runtime_allocations (
    workspace_local_id TEXT PRIMARY KEY NOT NULL REFERENCES remote_workspaces(workspace_local_id),
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    workflow_id TEXT NOT NULL REFERENCES cloud_workflows(workflow_id),
    job_id TEXT NOT NULL
) STRICT",
    "CREATE UNIQUE INDEX remote_runtime_allocations_workflow ON remote_runtime_allocations(workflow_id)",
    "CREATE UNIQUE INDEX remote_runtime_allocations_job ON remote_runtime_allocations(job_id)",
];

const FIRST_PIN_SCHEMA: &str = r"CREATE TABLE remote_first_pin_intents (
    workspace_local_id TEXT PRIMARY KEY NOT NULL REFERENCES remote_runtime_allocations(workspace_local_id),
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    workflow_id TEXT NOT NULL UNIQUE,
    job_id TEXT NOT NULL UNIQUE,
    version INTEGER NOT NULL CHECK (version = 1),
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64)
) STRICT, WITHOUT ROWID";

// Version-one selection rows are immutable intent, not provider ownership records.
const NETWORK_VOLUME_SCHEMA: [&str; 3] = [
    r"CREATE TABLE remote_network_volume_selections (
    workspace_local_id TEXT PRIMARY KEY NOT NULL REFERENCES remote_runtime_allocations(workspace_local_id),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36),
    generation INTEGER NOT NULL CHECK (generation > 0),
    workflow_id TEXT NOT NULL UNIQUE CHECK (length(workflow_id) = 36),
    job_id TEXT NOT NULL UNIQUE CHECK (length(job_id) = 36),
    version INTEGER NOT NULL CHECK (version = 1),
    volume_id TEXT NOT NULL CHECK (length(volume_id) BETWEEN 1 AND 191),
    data_center_id TEXT NOT NULL CHECK (length(data_center_id) BETWEEN 1 AND 191),
    minimum_size_gb INTEGER NOT NULL CHECK (minimum_size_gb BETWEEN 10 AND 4096),
    storage_type TEXT NOT NULL CHECK (storage_type = 'HIGH_PERFORMANCE')
) STRICT, WITHOUT ROWID",
    "CREATE TRIGGER remote_network_volume_selections_no_update BEFORE UPDATE ON remote_network_volume_selections BEGIN SELECT RAISE(ABORT, 'network volume selections are immutable'); END",
    "CREATE TRIGGER remote_network_volume_selections_no_delete BEFORE DELETE ON remote_network_volume_selections BEGIN SELECT RAISE(ABORT, 'network volume selections are immutable'); END",
];

// Frozen version-one identity binding, not provider observation or creation authority.
const PROVIDER_BINDING_SCHEMA: [&str; 4] = [
    r"CREATE TABLE remote_provider_bindings (
    workspace_local_id TEXT PRIMARY KEY NOT NULL REFERENCES remote_runtime_allocations(workspace_local_id),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36),
    generation INTEGER NOT NULL CHECK (generation > 0),
    workflow_id TEXT NOT NULL UNIQUE CHECK (length(workflow_id) = 36),
    job_id TEXT NOT NULL UNIQUE CHECK (length(job_id) = 36),
    version INTEGER NOT NULL CHECK (version = 1),
    provider TEXT NOT NULL CHECK (provider = 'azure'),
    subscription_id TEXT NOT NULL CHECK (length(subscription_id) = 36),
    profile_digest TEXT NOT NULL CHECK (length(CAST(profile_digest AS BLOB)) = 64 AND length(profile_digest) = 64 AND profile_digest NOT GLOB '*[^0-9a-f]*')
) STRICT, WITHOUT ROWID",
    "CREATE TRIGGER remote_provider_bindings_no_replace BEFORE INSERT ON remote_provider_bindings WHEN EXISTS(SELECT 1 FROM remote_provider_bindings WHERE workspace_local_id = NEW.workspace_local_id OR workflow_id = NEW.workflow_id OR job_id = NEW.job_id) BEGIN SELECT RAISE(ABORT, 'provider binding already exists'); END",
    "CREATE TRIGGER remote_provider_bindings_no_update BEFORE UPDATE ON remote_provider_bindings BEGIN SELECT RAISE(ABORT, 'provider bindings are immutable'); END",
    "CREATE TRIGGER remote_provider_bindings_no_delete BEFORE DELETE ON remote_provider_bindings BEGIN SELECT RAISE(ABORT, 'provider bindings are immutable'); END",
];

pub(super) fn open_connection(path: &Path) -> Result<Connection, CloudStoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(connection)
}

pub(super) fn open_read_connection(path: &Path) -> Result<Connection, CloudStoreError> {
    validate_read_path(path)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}

fn validate_read_path(path: &Path) -> Result<(), CloudStoreError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(CloudStoreError::SymlinkStorePath);
    }
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if parent.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(CloudStoreError::InsecureStoreDirectory);
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CloudStoreError::InsecureStoreFile);
        }
    }
    Ok(())
}

pub(super) fn initialize_schema(connection: &mut Connection) -> Result<(), CloudStoreError> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if !(0..=STORE_SCHEMA_VERSION).contains(&version) {
        return Err(CloudStoreError::UnsupportedSchema(version));
    }
    transaction.execute_batch(SCHEMA)?;
    if version < 2 {
        transaction.execute_batch(REMOTE_WORKSPACE_SCHEMA)?;
    }
    if version < 3 {
        for definition in REMOTE_ALLOCATION_SCHEMA {
            transaction.execute_batch(definition)?;
        }
    }
    drop(transaction.prepare(
        "SELECT workspace_local_id, session_id, revision, snapshot
         FROM remote_workspaces INDEXED BY remote_workspaces_session LIMIT 0",
    )?);
    validate_allocation_schema(&transaction)?;
    if version < 4 {
        creation_fences::migrate(&transaction)?;
    }
    creation_fences::validate_schema(&transaction)?;
    if version < 5 {
        // Existing allocations never acquire first-use provenance through migration.
        transaction.execute_batch(FIRST_PIN_SCHEMA)?;
    }
    validate_first_pin_schema(&transaction)?;
    if version < 6 {
        // Never infer a selection from legacy allocations, targets or profile names.
        for definition in NETWORK_VOLUME_SCHEMA {
            transaction.execute_batch(definition)?;
        }
    }
    validate_network_volume_schema(&transaction)?;
    if version < 7 {
        // Legacy profile names never imply an approved subscription or placement.
        validate_no_provider_binding_schema(&transaction)?;
        for definition in PROVIDER_BINDING_SCHEMA {
            transaction.execute_batch(definition)?;
        }
    }
    validate_provider_binding_schema(&transaction)?;
    transaction.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn ensure_current_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let version = connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    let legacy_read = matches!(version, 4..=6) && connection.is_readonly(rusqlite::MAIN_DB)?;
    if version != STORE_SCHEMA_VERSION && !legacy_read {
        return Err(CloudStoreError::UnsupportedSchema(version));
    }
    validate_allocation_schema(connection)?;
    creation_fences::validate_schema(connection)?;
    if version < 7 {
        validate_no_provider_binding_schema(connection)?;
    } else {
        validate_provider_binding_schema(connection)?;
    }
    if version < 6 {
        let partial: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema
             WHERE tbl_name = 'remote_network_volume_selections' COLLATE NOCASE
                OR name LIKE 'remote_network_volume_selections%')",
            [],
            |row| row.get(0),
        )?;
        if partial {
            return Err(CloudStoreError::InvalidAllocationSchema);
        }
    }
    if version == 4 {
        // Existing inventory must remain readable without migration. Partial
        // first-pin metadata is never treated as compatible legacy storage.
        let partial: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema
             WHERE tbl_name = 'remote_first_pin_intents' COLLATE NOCASE
                OR name = 'remote_first_pin_intents' COLLATE NOCASE)",
            [],
            |row| row.get(0),
        )?;
        return if partial {
            Err(CloudStoreError::InvalidAllocationSchema)
        } else {
            Ok(())
        };
    }
    validate_first_pin_schema(connection)?;
    if version >= 6 {
        validate_network_volume_schema(connection)?;
    }
    Ok(())
}

fn validate_no_provider_binding_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let partial: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema
         WHERE tbl_name = 'remote_provider_bindings' COLLATE NOCASE
            OR name LIKE 'remote!_provider!_bindings%' ESCAPE '!')",
        [],
        |row| row.get(0),
    )?;
    if partial {
        return Err(CloudStoreError::InvalidAllocationSchema);
    }
    Ok(())
}

fn validate_provider_binding_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let matches: bool = connection.query_row(
        "SELECT COUNT(*) = 4 AND COUNT(CASE WHEN sql IN (?1, ?2, ?3, ?4) THEN 1 END) = 4
         FROM main.sqlite_schema WHERE (tbl_name = 'remote_provider_bindings' COLLATE NOCASE
             OR name LIKE 'remote!_provider!_bindings%' ESCAPE '!') AND sql IS NOT NULL",
        PROVIDER_BINDING_SCHEMA,
        |row| row.get(0),
    )?;
    if !matches {
        return Err(CloudStoreError::InvalidAllocationSchema);
    }
    Ok(())
}

fn validate_network_volume_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let matches: bool = connection.query_row(
        "SELECT COUNT(*) = 3 AND COUNT(CASE WHEN sql IN (?1, ?2, ?3) THEN 1 END) = 3
         FROM main.sqlite_schema WHERE tbl_name = 'remote_network_volume_selections' AND sql IS NOT NULL",
        NETWORK_VOLUME_SCHEMA,
        |row| row.get(0),
    )?;
    if !matches {
        return Err(CloudStoreError::InvalidAllocationSchema);
    }
    Ok(())
}

fn validate_first_pin_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let matches: bool = connection.query_row(
        "SELECT COUNT(*) = 1 AND COUNT(CASE WHEN sql = ?1 THEN 1 END) = 1
         FROM main.sqlite_schema WHERE tbl_name = 'remote_first_pin_intents' AND sql IS NOT NULL",
        [FIRST_PIN_SCHEMA],
        |row| row.get(0),
    )?;
    if !matches {
        return Err(CloudStoreError::InvalidAllocationSchema);
    }
    Ok(())
}

fn validate_allocation_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    // These owned CREATE definitions are versioned data; keep their normalized SQL stable.
    // Exact matching includes constraints that column/index-name probes cannot inspect.
    let matches: bool = connection.query_row(
        "SELECT COUNT(*) = 3 AND COUNT(CASE WHEN sql IN (?1, ?2, ?3) THEN 1 END) = 3
         FROM main.sqlite_schema WHERE tbl_name = 'remote_runtime_allocations' AND sql IS NOT NULL",
        REMOTE_ALLOCATION_SCHEMA,
        |row| row.get(0),
    )?;
    if !matches {
        return Err(CloudStoreError::InvalidAllocationSchema);
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn canonical_store_path(path: &Path) -> Result<PathBuf, CloudStoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()?
        .join(path.file_name().unwrap_or(path.as_os_str())))
}

pub(super) fn prepare_private_store(path: &Path) -> Result<PathBuf, CloudStoreError> {
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
    let path = canonical_store_path(path)?;
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

#[cfg(test)]
mod tests;
