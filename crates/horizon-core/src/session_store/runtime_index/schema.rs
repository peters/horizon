//! The checks that tell whether a file is a Horizon runtime index that this Horizon can use.
use std::collections::{BTreeSet, HashMap};

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
/// schema version, make: a table, a column or another object is missing, different or
/// extra. A save then sets it aside instead of failing on it every time, also in a
/// migration to a later version that creates an object with the name of an extra one.
fn check_schema(connection: &Connection, applied: &[&str]) -> std::result::Result<(), IndexError> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(&applied.concat())?;
    if Schema::read(connection)? != Schema::read(&expected)? {
        return Err(IndexError::Damaged(format!(
            "the index does not have the schema of version {}",
            applied.len()
        )));
    }
    Ok(())
}

/// The schema of an index without the objects that SQLite keeps for itself.
#[derive(Debug, PartialEq, Eq)]
struct Schema {
    /// The name and type of the columns of each table, in order.
    tables: HashMap<String, Vec<(String, String)>>,
    /// The type and name of every other object, such as an index.
    objects: BTreeSet<(String, String)>,
}

impl Schema {
    fn read(connection: &Connection) -> rusqlite::Result<Self> {
        const OWN: &str = r"name NOT LIKE 'sqlite\_%' ESCAPE '\'";
        let mut tables: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut select = connection.prepare(&format!(
            "SELECT t.name, c.name, c.type FROM sqlite_schema AS t JOIN pragma_table_info(t.name) AS c
             WHERE t.type = 'table' AND t.{OWN} ORDER BY t.name, c.cid"
        ))?;
        let mut query = select.query([])?;
        while let Some(row) = query.next()? {
            tables.entry(row.get(0)?).or_default().push((row.get(1)?, row.get(2)?));
        }
        let mut select = connection.prepare(&format!(
            "SELECT type, name FROM sqlite_schema WHERE type != 'table' AND {OWN}"
        ))?;
        let objects = select
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { tables, objects })
    }
}
