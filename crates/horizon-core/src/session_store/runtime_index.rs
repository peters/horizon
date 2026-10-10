//! The runtime index: a SQLite store of a session's board next to its `runtime.yaml`.
//! It keeps the workspaces, the panels, the environments of cloud workspaces, and the
//! park state and last status line of cloud panels, so that they can be read without
//! the whole board.
//!
//! `runtime.yaml` stays the fallback for one release. A save writes it first and then
//! records its digest in the index. A load uses the index while `runtime.yaml` is
//! missing or still has that digest. A YAML file that something else wrote, such as an
//! earlier Horizon, wins over the index. A load also falls back to `runtime.yaml` when
//! the index is missing, damaged or has no board yet, and a save sets a damaged index
//! aside and builds a new one. An index from a newer Horizon is never read or changed.
mod rows;
#[cfg(feature = "cloud-workspaces")]
mod status;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{SessionStore, current_unix_millis, stable_state_key};
use crate::error::{Error, Result};
use crate::runtime_state::{PanelState, RuntimeState, WorkspaceState};
use rows::{BoardRows, Encoded, Format};

#[cfg(feature = "cloud-workspaces")]
pub use status::{CloudPanelStatus, ParkedPanel};

/// Marks the file as a Horizon runtime index ("HRZI").
const APPLICATION_ID: i32 = 0x4852_5a49;
const SCHEMA_VERSION: i32 = 1;
/// The schema changes from each version to the next, in order. Never change a
/// released entry; add a new one and increase `SCHEMA_VERSION`.
const MIGRATIONS: &[&str] = &[SCHEMA_V1];
/// How long a save waits for another Horizon process that writes the same index.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);

const SCHEMA_V1: &str = "
CREATE TABLE board (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    format TEXT NOT NULL,
    data TEXT NOT NULL,
    yaml_digest TEXT,
    saved_at INTEGER NOT NULL
) STRICT;
CREATE TABLE workspaces (
    seq INTEGER PRIMARY KEY,
    local_id TEXT NOT NULL,
    name TEXT NOT NULL,
    environments TEXT,
    format TEXT NOT NULL,
    data TEXT NOT NULL
) STRICT;
CREATE INDEX workspaces_by_local_id ON workspaces (local_id);
CREATE TABLE panels (
    workspace_seq INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    local_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    format TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (workspace_seq, seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX panels_by_local_id ON panels (local_id);
CREATE TABLE cloud_panels (
    panel_local_id TEXT PRIMARY KEY,
    parked INTEGER NOT NULL,
    activity TEXT,
    exit_status INTEGER,
    quiet_for_seconds INTEGER,
    last_line TEXT,
    read_at INTEGER,
    updated_at INTEGER NOT NULL
) STRICT;
";

#[derive(Debug, thiserror::Error)]
enum IndexError {
    #[error("the runtime index has schema version {0}, newer than version {SCHEMA_VERSION} of this Horizon")]
    Newer(i32),
    #[error("the runtime index is damaged: {0}")]
    Damaged(String),
    #[error("runtime index: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    State(#[from] Error),
}

impl IndexError {
    /// Whether the file is not a usable Horizon index, as opposed to busy or unwritable.
    fn is_damage(&self) -> bool {
        match self {
            Self::Damaged(_) => true,
            Self::Sqlite(error) => matches!(
                error.sqlite_error_code(),
                Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
            ),
            Self::Newer(_) | Self::State(_) => false,
        }
    }
}

impl From<IndexError> for Error {
    fn from(error: IndexError) -> Self {
        match error {
            IndexError::State(error) => error,
            other => Self::State(other.to_string()),
        }
    }
}

/// The index that this process wrote last, kept open: a WAL database checkpoints and
/// syncs its file when its last connection closes, which a save must not wait for.
#[derive(Clone, Debug, Default)]
pub(super) struct IndexCache(Arc<Mutex<Slot>>);

#[derive(Debug, Default)]
struct Slot {
    open: Option<OpenIndex>,
    /// The last failure that was logged, so a failure that repeats is logged once.
    warning: Option<String>,
}

#[derive(Debug)]
struct OpenIndex {
    session_id: String,
    connection: Connection,
}

impl IndexCache {
    /// Runs `operation` on the index of `session_id` at `path`, which it opens or
    /// creates first. A damaged index is set aside once and replaced by a new one.
    fn with<T>(
        &self,
        path: &Path,
        session_id: &str,
        operation: impl Fn(&mut Connection) -> std::result::Result<T, IndexError>,
    ) -> std::result::Result<T, IndexError> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.run(path, session_id, &operation) {
            Err(error) if error.is_damage() => {
                slot.open = None;
                let aside = set_aside(path).map_err(Error::from)?;
                tracing::warn!(
                    "set the damaged runtime index of session {session_id} aside as {}: {error}",
                    aside.display()
                );
                slot.run(path, session_id, &operation)
            }
            result => result,
        }
    }

    /// Closes the index of `session_id`, as before its files are removed.
    pub(super) fn forget(&self, session_id: &str) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.open.as_ref().is_some_and(|open| open.session_id == session_id) {
            slot.open = None;
        }
    }

    fn report(&self, session_id: &str, error: Option<&IndexError>) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match error {
            Some(error) => {
                let message = error.to_string();
                if slot.warning.as_deref() != Some(message.as_str()) {
                    tracing::warn!("runtime.yaml of session {session_id} stays the saved record: {message}");
                    slot.warning = Some(message);
                }
            }
            None => slot.warning = None,
        }
    }
}

impl Slot {
    fn run<T>(
        &mut self,
        path: &Path,
        session_id: &str,
        operation: &impl Fn(&mut Connection) -> std::result::Result<T, IndexError>,
    ) -> std::result::Result<T, IndexError> {
        let open = match self.open.take() {
            Some(open) if open.session_id == session_id => open,
            _ => OpenIndex {
                session_id: session_id.to_owned(),
                connection: open_for_write(path)?,
            },
        };
        operation(&mut self.open.insert(open).connection)
    }
}

/// A board as the index holds it, with the digest of the `runtime.yaml` written with it.
struct StoredBoard {
    state: RuntimeState,
    yaml_digest: Option<String>,
}

impl SessionStore {
    /// Records `state` in the runtime index after its `yaml` was written. A failure is
    /// logged once and leaves `runtime.yaml`, which the next load then prefers, as the
    /// saved record.
    pub(super) fn index_runtime_state(&self, session_id: &str, state: &RuntimeState, yaml: &str) {
        let path = self.home.session_runtime_index_path(session_id);
        let digest = yaml_digest(yaml);
        let result = BoardRows::new(state).map_err(IndexError::from).and_then(|rows| {
            self.index
                .with(&path, session_id, |connection| write_board(connection, &rows, &digest))
        });
        self.index.report(session_id, result.err().as_ref());
    }

    /// Loads the board of `session_id` from the runtime index, or from `runtime.yaml`
    /// when the index is missing, unreadable, has no board, or `runtime.yaml` changed
    /// after the index recorded it. Changes neither store; SQLite can add the empty
    /// `-wal` and `-shm` files of an index that it reads.
    ///
    /// # Errors
    /// Returns an error if neither store has a readable board, or if the index is newer
    /// than this Horizon supports.
    pub(super) fn load_runtime(&self, session_id: &str) -> Result<Option<RuntimeState>> {
        let yaml = match fs::read_to_string(self.home.session_runtime_path(session_id)) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        match read_board(&self.home.session_runtime_index_path(session_id)) {
            Ok(Some(stored))
                if yaml
                    .as_deref()
                    .is_none_or(|text| stored.yaml_digest.as_deref() == Some(yaml_digest(text).as_str())) =>
            {
                return Ok(Some(stored.state));
            }
            Ok(Some(_)) => {
                tracing::info!("loading runtime.yaml of session {session_id}: it changed after the runtime index");
            }
            Ok(None) => {}
            Err(error @ IndexError::Newer(_)) => return Err(error.into()),
            Err(error) if yaml.is_none() => return Err(error.into()),
            Err(error) => tracing::warn!("loading runtime.yaml of session {session_id}: {error}"),
        }
        yaml.as_deref().map(RuntimeState::from_yaml).transpose()
    }
}

/// The digest of a `runtime.yaml` text, which tells whether the file changed after a save.
fn yaml_digest(yaml: &str) -> String {
    format!("fnv1a64:{}:{}", yaml.len(), stable_state_key(yaml))
}

fn header(connection: &Connection) -> std::result::Result<(i32, i32), IndexError> {
    let application_id = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok((application_id, version))
}

/// Refuses a file that is not a Horizon index and an index from a newer Horizon.
/// Returns whether the index has its tables, as opposed to an empty new file.
fn check_header(connection: &Connection) -> std::result::Result<bool, IndexError> {
    match header(connection)? {
        (APPLICATION_ID, version) if version > SCHEMA_VERSION => Err(IndexError::Newer(version)),
        (APPLICATION_ID, version) if version > 0 => Ok(true),
        (0, 0) => {
            let tables: i64 = connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
            if tables == 0 {
                Ok(false)
            } else {
                Err(IndexError::Damaged("the file is not a Horizon runtime index".into()))
            }
        }
        _ => Err(IndexError::Damaged("the file is not a Horizon runtime index".into())),
    }
}

fn open_for_read(path: &Path) -> std::result::Result<Option<Connection>, IndexError> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    if !check_header(&connection)? {
        return Ok(None);
    }
    Ok(Some(connection))
}

fn open_for_write(path: &Path) -> std::result::Result<Connection, IndexError> {
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    check_header(&connection)?;
    // WAL commits are atomic without a sync, and the index is not the only record while
    // runtime.yaml is still written. A file system without WAL keeps a synced rollback journal.
    let journal: String = connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    let synchronous = if journal.eq_ignore_ascii_case("wal") {
        "NORMAL"
    } else {
        "FULL"
    };
    connection.pragma_update(None, "synchronous", synchronous)?;
    migrate(&mut connection)?;
    Ok(connection)
}

/// Brings the schema to `SCHEMA_VERSION`, in one transaction, so that two processes
/// that open a new index at the same time do not both create it.
fn migrate(connection: &mut Connection) -> std::result::Result<(), IndexError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_header(&transaction)?;
    let (_, version) = header(&transaction)?;
    let applied = usize::try_from(version).map_err(|_| IndexError::Damaged(format!("schema version {version}")))?;
    if applied < MIGRATIONS.len() {
        for migration in MIGRATIONS.iter().skip(applied) {
            transaction.execute_batch(migration)?;
        }
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    Ok(())
}

/// Moves a damaged index and the journal files beside it to a name with the time, so
/// that it is kept and a new index can take its place. The journal files move first:
/// a journal left beside a new index would be applied to it.
fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let aside = with_suffix(path, &format!(".damaged-{}", current_unix_millis()));
    for suffix in ["-wal", "-shm", "-journal", ""] {
        match fs::rename(with_suffix(path, suffix), with_suffix(&aside, suffix)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(aside)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn seq(value: usize) -> std::result::Result<i64, IndexError> {
    i64::try_from(value).map_err(|_| IndexError::State(Error::State("too many rows for the runtime index".into())))
}

fn stored(format: &str, data: String) -> Result<Encoded> {
    Ok(Encoded {
        format: Format::parse(format)?,
        data,
    })
}

/// The row for `value` when it differs from `stored`.
fn changed<T: Serialize + DeserializeOwned>(value: &T, stored: Option<&Encoded>) -> Result<Option<Encoded>> {
    let json = Encoded::json(value)?;
    if stored.is_some_and(|stored| stored.format == Format::Json && stored.data == json) {
        return Ok(None);
    }
    let encoded = Encoded::settle(value, json)?;
    Ok((stored != Some(&encoded)).then_some(encoded))
}

/// Writes the rows of a board that differ from the stored ones, in one transaction.
fn write_board(
    connection: &mut Connection,
    rows: &BoardRows<'_>,
    yaml_digest: &str,
) -> std::result::Result<(), IndexError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous = transaction
        .query_row("SELECT format, data FROM board WHERE id = 1", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .optional()?
        .map(|(format, data)| stored(&format, data))
        .transpose()?;
    match changed(&rows.board, previous.as_ref())? {
        Some(board) => transaction.execute(
            "INSERT OR REPLACE INTO board (id, format, data, yaml_digest, saved_at) VALUES (1, ?1, ?2, ?3, ?4)",
            params![board.format.as_str(), board.data, yaml_digest, current_unix_millis()],
        )?,
        None => transaction.execute(
            "UPDATE board SET yaml_digest = ?1, saved_at = ?2 WHERE id = 1",
            params![yaml_digest, current_unix_millis()],
        )?,
    };
    write_workspaces(&transaction, rows)?;
    write_panels(&transaction, rows)?;
    // Cloud panels that left the board keep no status.
    transaction.execute(
        "DELETE FROM cloud_panels WHERE panel_local_id NOT IN (SELECT local_id FROM panels)",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

fn write_workspaces(transaction: &Connection, rows: &BoardRows<'_>) -> std::result::Result<(), IndexError> {
    let mut stored_rows = HashMap::new();
    let mut select = transaction.prepare_cached("SELECT seq, format, data, environments FROM workspaces")?;
    let mut query = select.query([])?;
    while let Some(row) = query.next()? {
        let key: i64 = row.get(0)?;
        let encoded = stored(&row.get::<_, String>(1)?, row.get(2)?)?;
        stored_rows.insert(key, (encoded, row.get::<_, Option<String>>(3)?));
    }
    let mut insert = transaction.prepare_cached(
        "INSERT OR REPLACE INTO workspaces (seq, local_id, name, environments, format, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (index, workspace) in rows.workspaces.iter().enumerate() {
        let key = seq(index)?;
        let previous = stored_rows.get(&key);
        let data = changed(&workspace.state, previous.map(|(encoded, _)| encoded))?;
        let same_environments = previous.is_some_and(|(_, environments)| *environments == workspace.environments);
        let encoded = match (data, previous) {
            (Some(encoded), _) => encoded,
            (None, Some((encoded, _))) if !same_environments => encoded.clone(),
            (None, _) => continue,
        };
        insert.execute(params![
            key,
            workspace.state.local_id,
            workspace.state.name,
            workspace.environments,
            encoded.format.as_str(),
            encoded.data
        ])?;
    }
    transaction.execute("DELETE FROM workspaces WHERE seq >= ?1", [seq(rows.workspaces.len())?])?;
    Ok(())
}

fn write_panels(transaction: &Connection, rows: &BoardRows<'_>) -> std::result::Result<(), IndexError> {
    let mut stored_rows = HashMap::new();
    let mut select = transaction.prepare_cached("SELECT workspace_seq, seq, format, data FROM panels")?;
    let mut query = select.query([])?;
    while let Some(row) = query.next()? {
        let key: (i64, i64) = (row.get(0)?, row.get(1)?);
        stored_rows.insert(key, stored(&row.get::<_, String>(2)?, row.get(3)?)?);
    }
    let mut insert = transaction.prepare_cached(
        "INSERT OR REPLACE INTO panels (workspace_seq, seq, local_id, kind, name, format, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (workspace_index, workspace) in rows.workspaces.iter().enumerate() {
        for (panel_index, panel) in workspace.panels.iter().enumerate() {
            let key = (seq(workspace_index)?, seq(panel_index)?);
            let previous = stored_rows.remove(&key);
            let Some(encoded) = changed(panel, previous.as_ref())? else {
                continue;
            };
            let kind = serde_json::to_value(panel.kind).map_err(|error| Error::State(error.to_string()))?;
            insert.execute(params![
                key.0,
                key.1,
                panel.local_id,
                kind.as_str().unwrap_or_default(),
                panel.name,
                encoded.format.as_str(),
                encoded.data
            ])?;
        }
    }
    let mut delete = transaction.prepare_cached("DELETE FROM panels WHERE workspace_seq = ?1 AND seq = ?2")?;
    for (workspace_seq, panel_seq) in stored_rows.into_keys() {
        delete.execute([workspace_seq, panel_seq])?;
    }
    Ok(())
}

fn read_board(path: &Path) -> std::result::Result<Option<StoredBoard>, IndexError> {
    let Some(mut connection) = open_for_read(path)? else {
        return Ok(None);
    };
    // One read transaction gives a consistent board while another process writes.
    let transaction = connection.transaction()?;
    let Some((format, data, yaml_digest)) = transaction
        .query_row("SELECT format, data, yaml_digest FROM board WHERE id = 1", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?
    else {
        return Ok(None);
    };
    let board: RuntimeState = stored(&format, data)?.decode()?;

    let mut workspaces: Vec<WorkspaceState> = Vec::new();
    let mut select = transaction.prepare("SELECT seq, format, data FROM workspaces ORDER BY seq")?;
    let mut query = select.query([])?;
    while let Some(row) = query.next()? {
        if row.get::<_, i64>(0)? != seq(workspaces.len())? {
            return Err(IndexError::Damaged("the workspaces of the board have a gap".into()));
        }
        workspaces.push(stored(&row.get::<_, String>(1)?, row.get(2)?)?.decode()?);
    }

    let mut panels: Vec<(usize, PanelState)> = Vec::new();
    let mut counts = vec![0_usize; workspaces.len()];
    let mut select =
        transaction.prepare("SELECT workspace_seq, seq, format, data FROM panels ORDER BY workspace_seq, seq")?;
    let mut query = select.query([])?;
    while let Some(row) = query.next()? {
        let workspace = usize::try_from(row.get::<_, i64>(0)?)
            .ok()
            .filter(|index| *index < counts.len());
        let Some(workspace) = workspace else {
            return Err(IndexError::Damaged(
                "a panel names a workspace that the board does not have".into(),
            ));
        };
        if row.get::<_, i64>(1)? != seq(counts[workspace])? {
            return Err(IndexError::Damaged("the panels of a workspace have a gap".into()));
        }
        counts[workspace] += 1;
        panels.push((workspace, stored(&row.get::<_, String>(2)?, row.get(3)?)?.decode()?));
    }

    let state = rows::assemble(board, workspaces, panels)?.into_current()?;
    Ok(Some(StoredBoard { state, yaml_digest }))
}
