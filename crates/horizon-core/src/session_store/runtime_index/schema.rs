//! The checks that tell whether a file is a Horizon runtime index that this Horizon can use.
use std::collections::BTreeSet;

use rusqlite::Connection;

use super::{APPLICATION_ID, IndexError};

pub(super) fn header(connection: &Connection) -> std::result::Result<(i32, i32), IndexError> {
    let application_id = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok((application_id, version))
}

/// Refuses a file that is not a Horizon index, an index from a newer Horizon, and an
/// index without the schema of the version that it reports, current or older.
/// Returns whether the index has its tables, as opposed to an empty new file.
pub(super) fn check_header(connection: &Connection, migrations: &[&str]) -> std::result::Result<bool, IndexError> {
    let (application_id, version) = header(connection)?;
    match (application_id, usize::try_from(version)) {
        (APPLICATION_ID, Ok(applied)) if applied > migrations.len() => Err(IndexError::Newer(version)),
        (APPLICATION_ID, Ok(applied)) if applied > 0 => check_schema(connection, &migrations[..applied]).map(|()| true),
        (0, Ok(0)) => {
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

/// Refuses an index whose schema is not the one that `applied`, the migrations of its
/// schema version, make: a table, an index or another object is missing, extra, or has
/// another definition, such as other columns, keys or constraints. A save then sets it
/// aside instead of failing on it every time, also in a migration to a later version that
/// creates an object with the name of an extra one.
fn check_schema(connection: &Connection, applied: &[&str]) -> std::result::Result<(), IndexError> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(&applied.concat())?;
    if schema(connection)? != schema(&expected)? {
        return Err(IndexError::Damaged(format!(
            "the index does not have the schema of version {}",
            applied.len()
        )));
    }
    Ok(())
}

/// The type, name and table of an object, and the SQL that defines it.
type Object = (String, String, String, Option<String>);

/// Each object of an index, without the objects that SQLite keeps for itself. SQLite keeps
/// each definition as the migration wrote it, and updates it for a change such as
/// `ALTER TABLE`, so the same migrations give the same definitions. A migration is never
/// changed after a release.
fn schema(connection: &Connection) -> rusqlite::Result<BTreeSet<Object>> {
    let mut select = connection
        .prepare(r"SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite\_%' ESCAPE '\'")?;
    select
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect()
}
