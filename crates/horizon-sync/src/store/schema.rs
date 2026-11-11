//! The file of the server store: its pragmas, its tables and its schema version.
//!
//! One file holds everything that the server knows. It runs in WAL mode with
//! `synchronous=FULL`, so a committed change survives a power loss of the server
//! and an open reader never blocks the writer. The meta table names the store and
//! the schema version; a version that this Horizon does not know is refused, never
//! migrated down.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::StoreError;

/// The schema version that this Horizon writes.
pub(super) const SCHEMA_VERSION: i64 = 1;

/// How long a connection waits for another connection of the same file.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The pragmas of the one write connection.
const PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
";

/// Version 1 of the file: the meta table, the log of decided intents, the
/// identifiers that the server already applied, the objects, and the computers
/// that have sent an intent.
const SCHEMA_V1: &str = "
CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
CREATE TABLE log (
    idx INTEGER PRIMARY KEY,
    at_millis INTEGER NOT NULL,
    device TEXT NOT NULL,
    intent INTEGER NOT NULL,
    kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    result_json TEXT NOT NULL
) STRICT;
CREATE TABLE applied (
    device TEXT NOT NULL,
    intent INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    UNIQUE (device, intent)
) STRICT;
CREATE TABLE objects (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    body_json TEXT NOT NULL,
    version INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, id)
) STRICT;
CREATE INDEX objects_by_version ON objects (version);
CREATE TABLE devices (
    name TEXT PRIMARY KEY,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    last_intent INTEGER NOT NULL
) STRICT;
";

/// The changes from each schema version to the next, in order. Version 1 is the
/// first file that this crate writes, so no change is listed yet. Never change an
/// entry that has shipped; add a new one and increase [`SCHEMA_VERSION`].
const MIGRATIONS: &[&str] = &[];

/// The key of the store id in the meta table.
const META_STORE_ID: &str = "store_id";

/// The key of the schema version in the meta table.
const META_SCHEMA_VERSION: &str = "schema_version";

/// Opens the write connection with the pragmas of the store.
///
/// # Errors
/// Returns why the file cannot be opened or configured.
pub(super) fn open(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path)?;
    connection.execute_batch(PRAGMAS)?;
    Ok(connection)
}

/// Opens a read connection: no pragma that writes, so a reader never waits for
/// the writer and never blocks it.
///
/// # Errors
/// Returns why the file cannot be opened.
pub(super) fn read_only(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}

/// Creates the tables of a new file, or checks the version of an existing one and
/// migrates it when it is older. Returns the store id.
///
/// # Errors
/// Refuses a file from a newer Horizon, and reports a file that is not a server
/// store.
pub(super) fn prepare(connection: &mut Connection) -> Result<String, StoreError> {
    if !has_table(connection, "meta")? {
        let transaction = connection.transaction()?;
        transaction.execute_batch(SCHEMA_V1)?;
        let store_id = uuid::Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2), (?3, ?4)",
            params![META_STORE_ID, store_id, META_SCHEMA_VERSION, SCHEMA_VERSION.to_string()],
        )?;
        transaction.commit()?;
        return Ok(store_id);
    }
    let version = meta(connection, META_SCHEMA_VERSION)?
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| StoreError::Damaged("the meta table holds no schema version".to_owned()))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::Newer {
            found: version,
            expected: SCHEMA_VERSION,
        });
    }
    if version < SCHEMA_VERSION {
        migrate(connection, version)?;
    }
    meta(connection, META_STORE_ID)?.ok_or_else(|| StoreError::Damaged("the meta table holds no store id".to_owned()))
}

/// Runs the schema changes from `from` to [`SCHEMA_VERSION`].
fn migrate(connection: &mut Connection, from: i64) -> Result<(), StoreError> {
    let first = usize::try_from(from).map_or(MIGRATIONS.len(), |from| from.saturating_sub(1));
    let transaction = connection.transaction()?;
    for change in MIGRATIONS.iter().skip(first) {
        transaction.execute_batch(change)?;
    }
    transaction.execute(
        "UPDATE meta SET value = ?1 WHERE key = ?2",
        params![SCHEMA_VERSION.to_string(), META_SCHEMA_VERSION],
    )?;
    transaction.commit()?;
    Ok(())
}

/// One value of the meta table.
///
/// # Errors
/// Returns why the table cannot be read.
pub(super) fn meta(connection: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    connection
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
        .optional()
        .map_err(StoreError::from)
}

/// The last committed index, or 0 for an empty log.
///
/// # Errors
/// Returns why the log cannot be read.
pub(super) fn index_of(connection: &Connection) -> Result<u64, StoreError> {
    let index: i64 = connection.query_row("SELECT COALESCE(MAX(idx), 0) FROM log", [], |row| row.get(0))?;
    u64::try_from(index).map_err(|_| StoreError::Damaged(format!("the log index {index} is not one")))
}

/// Ends the write-ahead log of a closing store, so a copy of the file alone is
/// the whole store.
pub(super) fn checkpoint(connection: &Connection) {
    if let Err(error) = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
        tracing::warn!(%error, "the server store could not checkpoint its log");
    }
}

/// Whether the file has a table of that name.
fn has_table(connection: &Connection, name: &str) -> Result<bool, StoreError> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}
