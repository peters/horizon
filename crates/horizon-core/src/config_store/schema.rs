//! The tables of the settings store.
//!
//! Each table is `STRICT`, so a value of the wrong type is refused instead of
//! converted. A store carries its schema version in `meta`, and a Horizon that
//! reads another version refuses the file instead of reading it partially.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// The schema that this build writes and reads.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

pub(super) fn create(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             ) STRICT;
             CREATE TABLE IF NOT EXISTS settings (
                 section TEXT PRIMARY KEY,
                 format TEXT NOT NULL,
                 data TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             ) STRICT;",
        )
        .map_err(|error| Error::State(format!("settings store schema: {error}")))
}
