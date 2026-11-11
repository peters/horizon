//! The objects that a pull reports, and the index that they belong to.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::StoreError;
use crate::store::schema::index_of;

/// One object of the server, as a pull reports it.
///
/// A removed object stays as a row with `deleted: true`, so a computer that was
/// away removes it from its mirror instead of keeping an old copy of it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub kind: String,
    pub id: String,
    pub body: Value,
    /// The log index of the change that wrote this row.
    pub version: u64,
    pub deleted: bool,
}

/// The objects that changed after an index, and the state of the server that they
/// belong to.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    /// The last committed index of the server.
    pub index: u64,
    /// Whether the computer must replace its mirror with these rows, because the
    /// server is not the log that the computer held.
    pub reset: bool,
    /// The rows, in the order that their versions were committed.
    pub rows: Vec<Row>,
}

/// The rows that changed after `since`, and the index of the server.
///
/// # Errors
/// Returns why the objects cannot be read.
pub(super) fn changes(connection: &Connection, since: u64) -> Result<Changes, StoreError> {
    let rows = rows(connection, since)?;
    Ok(Changes {
        index: index_of(connection)?,
        reset: false,
        rows,
    })
}

/// Every row of the store, for a computer that must replace its mirror.
///
/// # Errors
/// Returns why the objects cannot be read.
pub(super) fn all(connection: &Connection) -> Result<Changes, StoreError> {
    Ok(Changes {
        index: index_of(connection)?,
        reset: true,
        rows: rows(connection, 0)?,
    })
}

/// The rows whose version is above `since`.
fn rows(connection: &Connection, since: u64) -> Result<Vec<Row>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT kind, id, body_json, version, deleted FROM objects WHERE version > ?1 ORDER BY version, kind, id",
    )?;
    let stored = statement
        .query_map([to_sql(since)?], |row| {
            Ok(StoredRow {
                kind: row.get(0)?,
                id: row.get(1)?,
                body_json: row.get(2)?,
                version: row.get(3)?,
                deleted: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    stored.into_iter().map(StoredRow::into_row).collect()
}

/// A count of intents or an index as the file holds it. A value above `i64::MAX`
/// is not one that this server can hold.
pub(super) fn to_sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Damaged(format!("the number {value} is not one that the store holds")))
}

/// One row as the file holds it.
struct StoredRow {
    kind: String,
    id: String,
    body_json: String,
    version: i64,
    deleted: i64,
}

impl StoredRow {
    fn into_row(self) -> Result<Row, StoreError> {
        let version = u64::try_from(self.version)
            .map_err(|_| StoreError::Damaged(format!("the version {} is not one", self.version)))?;
        let body = serde_json::from_str(&self.body_json)
            .map_err(|error| StoreError::Damaged(format!("the body of {} is not JSON: {error}", self.id)))?;
        Ok(Row {
            kind: self.kind,
            id: self.id,
            body,
            version,
            deleted: self.deleted != 0,
        })
    }
}
