use std::path::Path;

use crate::error::{Error, Result};
use crate::local_store::{grok_sessions_db_path, open_read_only_sqlite};

use super::{AgentSessionRecord, PanelKind, normalize_cwd};

/// Cap on catalog rows: the picker and binding bootstrap only consume
/// recent sessions, and unbounded tables would slow every startup.
const MAX_SESSION_ROWS: i64 = 1000;

/// Load Grok sessions from the CLI-maintained local search index.
///
/// Unlike Claude/Codex/Pi, Grok organizes its on-disk session tree under
/// URL-encoded working directories, so the raw `cwd` is not recoverable by
/// walking the tree. The `session_docs` index in `session_search.sqlite`
/// carries the decoded cwd and is the local source of truth Horizon reads.
pub(super) fn load_grok_sessions() -> Result<Vec<AgentSessionRecord>> {
    let Some(db_path) = grok_sessions_db_path() else {
        return Ok(Vec::new());
    };
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    load_grok_sessions_from_path(&db_path)
}

pub(super) fn load_grok_sessions_from_path(db_path: &Path) -> Result<Vec<AgentSessionRecord>> {
    let connection = open_read_only_sqlite(db_path)?;
    let table_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_docs'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| Error::State(error.to_string()))?;
    // Older or differently configured CLI builds may not maintain the
    // search index; treat a missing table as "no sessions", not a failure.
    if table_rows == 0 {
        return Ok(Vec::new());
    }

    let mut statement = connection
        .prepare(&format!(
            "SELECT session_id, cwd, updated_at, title
             FROM session_docs
             ORDER BY updated_at DESC
             LIMIT {MAX_SESSION_ROWS}"
        ))
        .map_err(|error| Error::State(error.to_string()))?;

    let rows = statement
        .query_map([], |row| {
            Ok(AgentSessionRecord {
                kind: PanelKind::Grok,
                session_id: row.get(0)?,
                label: row
                    .get::<_, String>(3)
                    .ok()
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| Some("Grok session".to_string())),
                cwd: normalize_cwd(row.get::<_, String>(1).ok().as_deref()),
                updated_at: normalize_updated_at(row.get::<_, i64>(2)?),
                interactive: true,
            })
        })
        .map_err(|error| Error::State(error.to_string()))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|error| Error::State(error.to_string()))?);
    }
    Ok(sessions)
}

/// The CLI does not document the unit of `session_docs.updated_at`. Horizon
/// compares catalog timestamps against panel launch times in milliseconds,
/// so normalize epoch-second values to milliseconds before they leave this
/// module. Any real-world second timestamp is far below the threshold, and
/// any real-world millisecond timestamp is far above it.
fn normalize_updated_at(value: i64) -> i64 {
    const SECOND_TIMESTAMP_THRESHOLD: u64 = 1_000_000_000_000;

    if value.unsigned_abs() < SECOND_TIMESTAMP_THRESHOLD {
        value.saturating_mul(1000)
    } else {
        value
    }
}
