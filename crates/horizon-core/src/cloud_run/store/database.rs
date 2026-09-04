//! Private database opening and schema boundary shared by control-plane records.

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use super::CloudStoreError;

const STORE_SCHEMA_VERSION: i64 = 1;
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

pub(super) fn initialize_schema(connection: &mut Connection) -> Result<(), CloudStoreError> {
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

pub(super) fn ensure_current_schema(connection: &Connection) -> Result<(), CloudStoreError> {
    let version = connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if version != STORE_SCHEMA_VERSION {
        return Err(CloudStoreError::UnsupportedSchema(version));
    }
    Ok(())
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
