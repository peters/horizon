//! Settings in one SQLite file.
//!
//! The store is the settings, and it is the only settings file that Horizon
//! reads or writes.
//!
//! This module owns only the file: one row for each configuration section, and
//! one `meta` row for the schema and the save time. The policy (which file,
//! which sections, when to import) lives in [`crate::Config`], so a host process
//! can use the same store with no copy next to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::config::Config;
use crate::error::{Error, Result};

mod schema;
mod sections;
#[cfg(test)]
mod tests;

pub use schema::SETTINGS_SCHEMA_VERSION;

/// The JSON value of one section of `config`, or `None` when this build has no
/// such section.
///
/// # Errors
/// Returns why the section cannot be encoded.
pub fn section_value(config: &Config, name: &str) -> Result<Option<serde_json::Value>> {
    sections::value_of(config, name).transpose()
}

/// The configuration of one set of raw rows of the store.
///
/// # Errors
/// Returns an error when a known section has data that this build cannot read.
pub fn decode_rows(rows: &BTreeMap<String, String>) -> Result<Config> {
    sections::decode(rows)
}

/// The row that holds the schema version of the store.
pub const META_SCHEMA_VERSION: &str = "schema_version";
/// The time of the last save, in milliseconds since the Unix epoch.
pub const META_SAVED_AT: &str = "saved_at";

/// The settings of a store.
#[derive(Clone, Debug)]
pub struct StoredSettings {
    pub config: Config,
}

/// The settings file of one configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigStore {
    path: PathBuf,
}

/// Why a store cannot give its settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreFault {
    /// The file is not a store that this build can read. The exported copy
    /// carries the same settings, so a load can use the copy and write the
    /// store again.
    Damaged(String),
    /// Another Horizon version wrote the store. This build must not replace it,
    /// and it must not use the copy in its place, because the store may hold
    /// newer settings.
    OtherVersion(String),
}

impl std::fmt::Display for StoreFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Damaged(reason) | Self::OtherVersion(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl ConfigStore {
    /// The store that holds the settings of `config_path`.
    ///
    /// The store is next to the settings file, so an isolated instance that
    /// names its own settings also names its own store: `config.json` and
    /// `horizon.json` use `settings.sqlite`, and another name uses
    /// `<name>.settings.sqlite`.
    #[must_use]
    pub fn resolve(config_path: &Path) -> Self {
        Self {
            path: store_path_for(config_path),
        }
    }

    /// A store at an exact path.
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The path of the store. It does not exist until the first save.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the settings, or `None` when the store does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error that names the store when the file exists but cannot be
    /// read. A store that cannot be read must not be mistaken for a store that
    /// has no settings, because that would replace the settings of a person with
    /// defaults.
    pub fn load(&self) -> Result<Option<StoredSettings>> {
        crate::ui_thread::blocking("the settings store is read");
        self.open().map_err(|fault| self.fault_error(&fault))
    }

    /// Read the settings, and name why a read failed, so a caller can use the
    /// exported copy of a damaged store but never of a store of another version.
    ///
    /// # Errors
    ///
    /// Returns [`StoreFault::Damaged`] when the file is not a store that this
    /// build can read, and [`StoreFault::OtherVersion`] when another Horizon
    /// version wrote it.
    pub fn open(&self) -> std::result::Result<Option<StoredSettings>, StoreFault> {
        crate::ui_thread::blocking("the settings store is read");
        if !self.path.is_file() {
            return Ok(None);
        }
        self.read().map(Some)
    }

    /// Move a damaged store aside, so a load can write a new one from the copy
    /// that carries the same settings. The damaged file stays next to the store,
    /// with a name that says what it is, for a person to examine.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be moved.
    pub fn move_damaged_aside(&self) -> Result<PathBuf> {
        crate::ui_thread::blocking("a damaged settings store is renamed");
        let name = self
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("settings.sqlite");
        let stamp = unix_millis() / 1000;
        let target = self.path.with_file_name(format!("{name}.damaged-{stamp}"));
        std::fs::rename(&self.path, &target)?;
        Ok(target)
    }

    /// The error of one store fault, with the path of the store.
    #[must_use]
    pub fn fault_error(&self, fault: &StoreFault) -> Error {
        let what = match fault {
            StoreFault::Damaged(_) => "is damaged",
            StoreFault::OtherVersion(_) => "cannot be read",
        };
        Error::State(format!("settings store {} {what}: {fault}", self.path.display()))
    }

    /// The raw JSON of every section of the store, by section name.
    ///
    /// This reads the rows as they are, so a section that this build does not know
    /// is in the answer too. A store that does not exist holds no row.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists and cannot be read.
    pub fn section_rows(&self) -> Result<BTreeMap<String, String>> {
        crate::ui_thread::blocking("the settings store is read");
        if !self.path.is_file() {
            return Ok(BTreeMap::new());
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| state_error(&error))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(BUSY_TIMEOUT_SECONDS))
            .map_err(|error| state_error(&error))?;
        read_rows(&connection)
    }

    /// Write the rows of `rows` and nothing else, so a section that this build
    /// does not touch keeps its row and its unknown fields.
    ///
    /// The caller has checked the configuration that these rows make; the store
    /// writes the JSON as it is given.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be written.
    pub fn write_sections(&self, rows: &BTreeMap<String, String>) -> Result<()> {
        crate::ui_thread::blocking("the settings store is committed");
        let mut connection = self.open_for_write()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| state_error(&error))?;
        let now = unix_millis();
        {
            let mut upsert = transaction
                .prepare(
                    "INSERT INTO settings (section, format, data, updated_at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(section) DO UPDATE SET \
                     format = excluded.format, data = excluded.data, updated_at = excluded.updated_at",
                )
                .map_err(|error| state_error(&error))?;
            for (section, data) in rows {
                upsert
                    .execute(params![section, sections::FORMAT, data, now])
                    .map_err(|error| state_error(&error))?;
            }
            set_meta(&transaction, META_SAVED_AT, &now.to_string())?;
        }
        transaction.commit().map_err(|error| state_error(&error))
    }

    /// Write every section of `config` in one transaction.
    ///
    /// A row of a section that this build does not know stays as it is, so a
    /// store that a newer Horizon wrote keeps that section when an older
    /// Horizon saves.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be written or a section cannot be
    /// encoded.
    pub fn save(&self, config: &Config) -> Result<()> {
        crate::ui_thread::blocking("the settings store is committed");
        let rows = sections::encode(config)?;
        let mut connection = self.open_for_write()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| state_error(&error))?;
        let now = unix_millis();
        {
            let mut upsert = transaction
                .prepare(
                    "INSERT INTO settings (section, format, data, updated_at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(section) DO UPDATE SET \
                     format = excluded.format, data = excluded.data, updated_at = excluded.updated_at",
                )
                .map_err(|error| state_error(&error))?;
            for row in &rows {
                upsert
                    .execute(params![row.section, sections::FORMAT, row.data, now])
                    .map_err(|error| state_error(&error))?;
            }
            set_meta(&transaction, META_SCHEMA_VERSION, &SETTINGS_SCHEMA_VERSION.to_string())?;
            set_meta(&transaction, META_SAVED_AT, &now.to_string())?;
        }
        transaction.commit().map_err(|error| state_error(&error))
    }

    fn read(&self) -> std::result::Result<StoredSettings, StoreFault> {
        let reason = |error: rusqlite::Error| StoreFault::Damaged(error.to_string());
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(reason)?;
        let version = meta(&connection, META_SCHEMA_VERSION)?;
        match version {
            Some(version) if version == SETTINGS_SCHEMA_VERSION.to_string() => {}
            Some(version) => {
                return Err(StoreFault::OtherVersion(format!(
                    "it has schema {version} and this Horizon reads schema {SETTINGS_SCHEMA_VERSION}"
                )));
            }
            None => return Err(StoreFault::Damaged("it has no schema version".to_string())),
        }
        let rows = read_rows(&connection).map_err(|error| StoreFault::Damaged(error.to_string()))?;
        // An older build kept a `mirror_sha256` row that hashed an exported
        // copy. This build writes no copy, so the row is inert: it is not read
        // and it is not deleted, so an older Horizon still finds its store.
        Ok(StoredSettings {
            config: sections::decode(&rows).map_err(|error| StoreFault::Damaged(error.to_string()))?,
        })
    }

    fn open_for_write(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&self.path).map_err(|error| state_error(&error))?;
        // One file at rest: the default rollback journal is removed at each
        // commit, so a copy of the store while Horizon does not run is whole.
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|error| state_error(&error))?;
        // Two writers of one store are normal: the settings worker of this window,
        // the worker of a link that applies a section of the server, and a second
        // window on the same home. Each write is short, so a writer waits for the
        // other one instead of failing with `database is locked`.
        connection
            .busy_timeout(std::time::Duration::from_secs(BUSY_TIMEOUT_SECONDS))
            .map_err(|error| state_error(&error))?;
        schema::create(&connection)?;
        Ok(connection)
    }
}

/// How long a connection of one store waits for the writer of another one.
const BUSY_TIMEOUT_SECONDS: u64 = 5;

/// The raw data of every row of the store, by section name.
fn read_rows(connection: &Connection) -> Result<BTreeMap<String, String>> {
    let mut rows = BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT section, format, data FROM settings")
        .map_err(|error| state_error(&error))?;
    let mut queried = statement.query([]).map_err(|error| state_error(&error))?;
    while let Some(row) = queried.next().map_err(|error| state_error(&error))? {
        let section: String = row.get(0).map_err(|error| state_error(&error))?;
        let format: String = row.get(1).map_err(|error| state_error(&error))?;
        let data: String = row.get(2).map_err(|error| state_error(&error))?;
        if format != sections::FORMAT {
            return Err(Error::State(format!("settings section {section} has format {format}")));
        }
        rows.insert(section, data);
    }
    Ok(rows)
}

fn store_path_for(config_path: &Path) -> PathBuf {
    let stem = config_path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("horizon");
    let name = match stem {
        "config" | "horizon" => "settings.sqlite".to_string(),
        other => format!("{other}.settings.sqlite"),
    };
    config_path.with_file_name(name)
}

fn meta(connection: &Connection, key: &str) -> std::result::Result<Option<String>, StoreFault> {
    let mut statement = connection
        .prepare("SELECT value FROM meta WHERE key = ?1")
        .map_err(|error| StoreFault::Damaged(error.to_string()))?;
    let mut rows = statement
        .query(params![key])
        .map_err(|error| StoreFault::Damaged(error.to_string()))?;
    match rows.next().map_err(|error| StoreFault::Damaged(error.to_string()))? {
        Some(row) => Ok(Some(
            row.get(0).map_err(|error| StoreFault::Damaged(error.to_string()))?,
        )),
        None => Ok(None),
    }
}

fn set_meta(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection
        .execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|error| state_error(&error))?;
    Ok(())
}

fn state_error(error: &rusqlite::Error) -> Error {
    Error::State(error.to_string())
}

fn unix_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
