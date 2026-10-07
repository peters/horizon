//! The state of agent sessions whose panels are parked. A parked panel has no
//! terminal attached, so Horizon reads a short status over SSH instead: whether the
//! session still runs, when it last wrote output and its last lines. The reader is
//! sent with the command, so any worker with Python 3 and tmux can answer it.
use super::{Error, Result, command::Runner, settings::Settings, ssh::Connection};
use base64::Engine;
use horizon_cloud::{Cancellation, Worker, valid_id};
use serde::Deserialize;
use std::{path::Path, time::Duration};

const SCRIPT: &str = include_str!("session_status.py");
const TIMEOUT: Duration = Duration::from_secs(30);
/// The most sessions one read asks for; the script answers no more than this.
pub const MAX_SESSIONS: usize = 64;
const MAX_LINES: usize = 8;
const MAX_LINE_CHARS: usize = 240;

/// What a session does, as its status shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionActivity {
    /// The agent shows its working indicator.
    Working,
    /// The process runs and shows no working indicator.
    Idle,
    /// The process ended with this status; the session keeps its output.
    Exited(Option<i32>),
    /// The worker has no session with this id.
    Missing,
}

/// The status of one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatus {
    pub id: String,
    pub activity: SessionActivity,
    /// Time since the session last wrote output, when the worker knows it.
    pub quiet_for: Option<Duration>,
    /// The last non-empty lines of the session's screen, oldest first.
    pub lines: Vec<String>,
}

impl SessionStatus {
    /// The last non-empty line of the screen.
    #[must_use]
    pub fn last_line(&self) -> Option<&str> {
        self.lines.last().map(String::as_str)
    }
}

#[derive(Deserialize)]
struct Response {
    sessions: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    state: String,
    #[serde(default)]
    exit_status: Option<i32>,
    #[serde(default)]
    activity_age_seconds: Option<u64>,
    #[serde(default)]
    lines: Vec<String>,
}

/// The remote command that prints the status of `ids`.
///
/// # Errors
/// Refuses an empty list, too many sessions and any id that is not a session id.
pub fn command(ids: &[String]) -> Result<String> {
    if ids.is_empty() || ids.len() > MAX_SESSIONS || !ids.iter().all(|id| valid_id(id)) {
        return Err(Error::Invalid("Invalid session ids for a status read"));
    }
    let script = base64::engine::general_purpose::STANDARD.encode(SCRIPT);
    Ok(format!(
        "printf %s '{script}' | base64 -d | python3 - {}",
        ids.join(" ")
    ))
}

/// Parses what the script printed. Sessions that were not asked for are dropped,
/// and every line is cut to a bounded length without control characters.
///
/// # Errors
/// Refuses output that is not one status object.
pub fn parse(output: &str, ids: &[String]) -> Result<Vec<SessionStatus>> {
    let response: Response =
        serde_json::from_str(output.trim()).map_err(|_| Error::Invalid("The worker's session status is malformed"))?;
    Ok(response
        .sessions
        .into_iter()
        .filter(|entry| ids.contains(&entry.id))
        .take(MAX_SESSIONS)
        .map(|entry| {
            let lines: Vec<String> = entry
                .lines
                .iter()
                .rev()
                .take(MAX_LINES)
                .rev()
                .map(|line| clean(line))
                .filter(|line| !line.is_empty())
                .collect();
            let activity = match entry.state.as_str() {
                "running" if lines.iter().any(|line| crate::agents::is_agent_working_line(line)) => {
                    SessionActivity::Working
                }
                "running" => SessionActivity::Idle,
                "exited" => SessionActivity::Exited(entry.exit_status),
                _ => SessionActivity::Missing,
            };
            SessionStatus {
                id: entry.id,
                activity,
                quiet_for: entry.activity_age_seconds.map(Duration::from_secs),
                lines,
            }
        })
        .collect())
}

fn clean(line: &str) -> String {
    line.chars()
        .filter(|c| !c.is_control())
        .take(MAX_LINE_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Reads the status of the sessions `ids` on `worker` over its pinned connection.
///
/// # Errors
/// Reports invalid ids, a failed connection and malformed output.
pub fn read(
    worker: &Worker,
    settings: &Settings,
    root: &Path,
    ids: &[String],
    cancel: &Cancellation,
) -> Result<Vec<SessionStatus>> {
    let remote = command(ids)?;
    let connection = Connection::new(worker, settings, root)?;
    let output = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Reading the status of parked sessions",
        &mut connection.pinned_command(&remote),
        TIMEOUT,
    )?;
    parse(&output, ids)
}

#[cfg(test)]
mod tests;
