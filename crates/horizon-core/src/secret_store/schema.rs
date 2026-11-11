//! The file of the secret values of one Horizon home: its pragmas, its tables and
//! its schema version.
//!
//! The schema itself is [`schema.sql`](schema.sql), because the smoke fixture of
//! the repository writes the same file with Python's `sqlite3` and must not
//! drift from this build. The file is made private before SQLite opens it, and
//! SQLite copies the mode of that file to the `-wal` and `-shm` files that it
//! makes, so the three are the owner's alone whatever the umask of the process
//! is.

use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::remote_browser_credential::RemoteCredentialError;

/// The schema version that this build writes.
pub const SECRETS_SCHEMA_VERSION: i64 = 1;

/// The version 1 tables, as the fixture and this build write them.
const SCHEMA_V1: &str = include_str!("schema.sql");

/// The mode of the file: the owner alone reads it.
const FILE_MODE: u32 = 0o600;

/// The mode of a directory that this build makes for the file.
const DIRECTORY_MODE: u32 = 0o700;

/// The pragmas of a connection. A write-ahead log lets a reader of the file (a
/// second Horizon window, or a tool) read while this build writes, and
/// `synchronous=FULL` keeps a committed value across a power loss.
const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
";

/// The key of the schema version in the meta table.
const META_SCHEMA_VERSION: &str = "schema_version";

/// Opens the write connection of `path`, makes the file private and creates its
/// tables.
///
/// # Errors
/// Returns the path and the reason when the folder or the file cannot be made,
/// made private or read.
pub(super) fn open(path: &Path) -> Result<Connection, RemoteCredentialError> {
    make_folder(path)?;
    make_private(path)?;
    let connection = Connection::open(path).map_err(|error| file_error(path, &error.to_string()))?;
    connection
        .execute_batch(PRAGMAS)
        .map_err(|error| file_error(path, &error.to_string()))?;
    prepare(&connection, path)?;
    Ok(connection)
}

/// Opens a store in memory, for a test and for a preview that must hold values
/// without a file of the person.
///
/// # Errors
/// Returns why the schema of a new file cannot be made.
pub(super) fn in_memory() -> Result<Connection, RemoteCredentialError> {
    let connection = Connection::open_in_memory().map_err(|error| RemoteCredentialError::File {
        path: Path::new(IN_MEMORY).to_path_buf(),
        reason: error.to_string(),
    })?;
    prepare(&connection, Path::new(IN_MEMORY))?;
    Ok(connection)
}

/// The name of the file that a store in memory reports.
pub(super) const IN_MEMORY: &str = ":memory:";

/// Creates the folder of a secrets file that is not there, private from the
/// first byte, and leaves an existing folder as it is.
fn make_folder(path: &Path) -> Result<(), RemoteCredentialError> {
    let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) else {
        return Ok(());
    };
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(DIRECTORY_MODE);
    builder
        .create(parent)
        .map_err(|error| file_error(path, &error.to_string()))
}

/// Makes the file of the store, or a file that an older build left readable,
/// reachable by its owner alone.
///
/// A file that is not there is made with `create_new` at [`FILE_MODE`], so it has
/// that mode from the first byte and the descriptor goes away before SQLite opens
/// the same file. A file that is there is corrected by its path: closing a plain
/// descriptor of a database releases every POSIX lock that this process holds on
/// it, and SQLite names that as a way to corrupt a database.
fn make_private(path: &Path) -> Result<(), RemoteCredentialError> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
    {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let mode = fs::metadata(path)
                .map_err(|error| file_error(path, &error.to_string()))?
                .permissions()
                .mode()
                & 0o777;
            if mode != FILE_MODE {
                fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))
                    .map_err(|error| file_error(path, &error.to_string()))?;
            }
            Ok(())
        }
        Err(error) => Err(file_error(path, &error.to_string())),
    }
}

fn file_error(path: &Path, reason: &str) -> RemoteCredentialError {
    RemoteCredentialError::File {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

/// Creates the tables of a new file, or checks the version of an existing one.
///
/// # Errors
/// Refuses a file of a newer build, and reports a file that is not a secrets
/// file.
pub(super) fn prepare(connection: &Connection, path: &Path) -> Result<(), RemoteCredentialError> {
    if !has_table(connection, path, "meta")? {
        connection
            .execute_batch(SCHEMA_V1)
            .map_err(|error| file_error(path, &error.to_string()))?;
        connection
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                params![META_SCHEMA_VERSION, SECRETS_SCHEMA_VERSION.to_string()],
            )
            .map_err(|error| file_error(path, &error.to_string()))?;
        return Ok(());
    }
    let version =
        schema_version(connection, path)?.ok_or_else(|| file_error(path, "its meta table holds no schema version"))?;
    if version != SECRETS_SCHEMA_VERSION {
        return Err(file_error(
            path,
            &format!("it has schema {version} and this build reads schema {SECRETS_SCHEMA_VERSION}"),
        ));
    }
    Ok(())
}

/// The schema version of a file, or `None` when its meta table has none.
///
/// # Errors
/// Returns the path and the reason when the meta table cannot be read.
pub(super) fn schema_version(connection: &Connection, path: &Path) -> Result<Option<i64>, RemoteCredentialError> {
    let stored: Option<String> = connection
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_SCHEMA_VERSION],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| file_error(path, &error.to_string()))?;
    Ok(stored.and_then(|value| value.parse().ok()))
}

/// Whether the file has a table of that name.
fn has_table(connection: &Connection, path: &Path, name: &str) -> Result<bool, RemoteCredentialError> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| file_error(path, &error.to_string()))?;
    Ok(found.is_some())
}
