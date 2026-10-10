//! The park state and the last status of cloud panels in the runtime index. A parked
//! panel has no terminal, so this is what Horizon knows of its session until the
//! worker is read again.
use std::time::Duration;

use rusqlite::{Row, TransactionBehavior, params};

use super::{IndexError, SessionStore, current_unix_millis, open_for_read};
use crate::cloud_runtime::session_status::{SessionActivity, SessionStatus};
use crate::error::Result;

/// A cloud panel as the board shows it now.
#[derive(Clone, Copy, Debug)]
pub struct ParkedPanel<'a> {
    pub local_id: &'a str,
    pub parked: bool,
    /// The status that the last read of the worker returned for the panel, if any.
    pub status: Option<&'a SessionStatus>,
}

/// What the runtime index keeps for a cloud panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudPanelStatus {
    pub panel_local_id: String,
    pub parked: bool,
    /// The last status that the worker returned. It stays after the panel attaches again
    /// and after a read that failed.
    pub activity: Option<SessionActivity>,
    pub quiet_for: Option<Duration>,
    pub last_line: Option<String>,
    /// When the status was read, in Unix milliseconds.
    pub read_at: Option<i64>,
}

impl SessionStore {
    /// Records the park state of `panels` of `session_id` and the status read for each.
    /// A panel without a status keeps the status recorded before.
    ///
    /// # Errors
    /// Returns an error if the runtime index cannot be opened or written.
    pub fn record_cloud_panels(&self, session_id: &str, panels: &[ParkedPanel<'_>]) -> Result<()> {
        let path = self.home.session_runtime_index_path(session_id);
        let now = current_unix_millis();
        self.index
            .with(&path, session_id, |connection| {
                let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                {
                    let mut park = transaction.prepare_cached(
                        "INSERT INTO cloud_panels (panel_local_id, parked, updated_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT (panel_local_id) DO UPDATE
                         SET parked = excluded.parked, updated_at = excluded.updated_at",
                    )?;
                    let mut status = transaction.prepare_cached(
                        "UPDATE cloud_panels
                         SET activity = ?2, exit_status = ?3, quiet_for_seconds = ?4, last_line = ?5, read_at = ?6
                         WHERE panel_local_id = ?1",
                    )?;
                    for panel in panels {
                        park.execute(params![panel.local_id, panel.parked, now])?;
                        if let Some(read) = panel.status {
                            let (activity, exit_status) = activity_columns(read.activity);
                            let quiet_for = read
                                .quiet_for
                                .map(|quiet| i64::try_from(quiet.as_secs()).unwrap_or(i64::MAX));
                            status.execute(params![
                                panel.local_id,
                                activity,
                                exit_status,
                                quiet_for,
                                read.last_line(),
                                now
                            ])?;
                        }
                    }
                }
                transaction.commit()?;
                Ok(())
            })
            .map_err(Into::into)
    }

    /// The park state and the last status of each cloud panel of `session_id`, by
    /// panel local id. Changes nothing in the index.
    ///
    /// # Errors
    /// Returns an error if the runtime index cannot be read.
    pub fn cloud_panel_statuses(&self, session_id: &str) -> Result<Vec<CloudPanelStatus>> {
        let Some(connection) = open_for_read(&self.home.session_runtime_index_path(session_id))? else {
            return Ok(Vec::new());
        };
        let mut select = connection
            .prepare(
                "SELECT panel_local_id, parked, activity, exit_status, quiet_for_seconds, last_line, read_at
                 FROM cloud_panels ORDER BY panel_local_id",
            )
            .map_err(IndexError::from)?;
        let statuses = select
            .query_map([], status_row)
            .and_then(Iterator::collect)
            .map_err(IndexError::from)?;
        Ok(statuses)
    }
}

fn activity_columns(activity: SessionActivity) -> (&'static str, Option<i32>) {
    match activity {
        SessionActivity::Working => ("working", None),
        SessionActivity::Idle => ("idle", None),
        SessionActivity::Exited(status) => ("exited", status),
        SessionActivity::Missing => ("missing", None),
    }
}

fn status_row(row: &Row<'_>) -> rusqlite::Result<CloudPanelStatus> {
    let activity = match row.get::<_, Option<String>>(2)?.as_deref() {
        Some("working") => Some(SessionActivity::Working),
        Some("idle") => Some(SessionActivity::Idle),
        Some("exited") => Some(SessionActivity::Exited(row.get(3)?)),
        Some("missing") => Some(SessionActivity::Missing),
        _ => None,
    };
    Ok(CloudPanelStatus {
        panel_local_id: row.get(0)?,
        parked: row.get(1)?,
        activity,
        quiet_for: row
            .get::<_, Option<i64>>(4)?
            .map(|seconds| Duration::from_secs(u64::try_from(seconds).unwrap_or_default())),
        last_line: row.get(5)?,
        read_at: row.get(6)?,
    })
}
