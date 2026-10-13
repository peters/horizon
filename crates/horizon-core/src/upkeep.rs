//! Upkeep: one agent on a cloud worker that keeps dependency pull requests moving.
//!
//! The worker's agent writes a status document after every change; the Upkeep board
//! panel only reads it. The document is external input: it is read off the UI thread,
//! size-limited, parsed into a fixed schema with bounded lists and texts, and never run.
//! Unknown fields are ignored, and unknown states show as "Other".
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

mod reader;

/// The time crate, so panels that show these times need no dependency of their own.
pub use ::time;

/// What the agent reports. Unknown fields are ignored so the worker can add some; a field of
/// the wrong type falls back to its default, and a list entry that does not parse is left out.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Status {
    #[serde(default, deserialize_with = "lenient")]
    pub agent: String,
    #[serde(default, deserialize_with = "lenient")]
    pub worker: Worker,
    #[serde(default, deserialize_with = "lenient")]
    pub schedule: Schedule,
    #[serde(default, deserialize_with = "lenient_list")]
    pub repositories: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub runs: Vec<Run>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub pull_requests: Vec<PullRequest>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub activity: Vec<Activity>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Worker {
    #[serde(default, deserialize_with = "lenient")]
    pub name: String,
    #[serde(default, deserialize_with = "lenient")]
    pub location: String,
    #[serde(default, deserialize_with = "lenient")]
    pub state: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Schedule {
    #[serde(default, deserialize_with = "lenient")]
    pub every_minutes: u32,
    #[serde(default, deserialize_with = "lenient_time")]
    pub next_run_at: Option<OffsetDateTime>,
    #[serde(default, deserialize_with = "lenient")]
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Done,
    Running,
    Skipped,
    Failed,
    /// No outcome, or one this Horizon does not know.
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Run {
    #[serde(deserialize_with = "rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(default, deserialize_with = "lenient_time")]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(default, deserialize_with = "lenient")]
    pub outcome: RunOutcome,
    #[serde(default, deserialize_with = "lenient")]
    pub merged: u32,
}

/// Where a pull request stands, as the agent sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// The agent cannot move it and a person must look.
    Blocked,
    /// The agent is fixing a failed check or a conflict.
    Fixing,
    /// Waits for a review the repository requires.
    NeedsReview,
    /// Checks still run.
    WaitingChecks,
    /// Every gate passed; the next run merges it.
    Ready,
    Merged,
    /// A state this Horizon does not know.
    #[default]
    #[serde(other)]
    Other,
}

impl PrState {
    pub const ORDER: [Self; 7] = [
        Self::Blocked,
        Self::Fixing,
        Self::NeedsReview,
        Self::WaitingChecks,
        Self::Ready,
        Self::Other,
        Self::Merged,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocked => "Blocked",
            Self::Fixing => "Fixing",
            Self::NeedsReview => "Needs review",
            Self::WaitingChecks => "Checks running",
            Self::Ready => "Ready to merge",
            Self::Merged => "Merged",
            Self::Other => "Other",
        }
    }

    /// Whether a person should look at it.
    #[must_use]
    pub fn needs_person(self) -> bool {
        matches!(self, Self::Blocked | Self::NeedsReview)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    Passed,
    Failed,
    Running,
    /// No checks, or a check state this Horizon does not know.
    #[default]
    #[serde(other)]
    None,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequest {
    pub repository: String,
    pub number: u64,
    #[serde(default, deserialize_with = "lenient")]
    pub title: String,
    #[serde(default, deserialize_with = "lenient")]
    pub url: String,
    #[serde(default, deserialize_with = "lenient")]
    pub author: String,
    #[serde(default, deserialize_with = "lenient")]
    pub checks: Checks,
    #[serde(default, deserialize_with = "lenient")]
    pub state: PrState,
    #[serde(default, deserialize_with = "lenient")]
    pub note: String,
    #[serde(default, deserialize_with = "lenient_time")]
    pub updated_at: Option<OffsetDateTime>,
}

impl PullRequest {
    /// The pull request page, only when the URL is exactly the GitHub page of this
    /// repository and number and the repository is a plain `owner/name`. Anything else
    /// the worker wrote is not opened.
    #[must_use]
    pub fn github_url(&self) -> Option<String> {
        if !plain_repository(&self.repository) {
            return None;
        }
        let expected = format!("https://github.com/{}/pull/{}", self.repository, self.number);
        (self.url == expected).then_some(expected)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Activity {
    #[serde(deserialize_with = "rfc3339")]
    pub at: OffsetDateTime,
    pub text: String,
}

/// Where the board reads the status document when its panel names none.
#[must_use]
pub fn default_status_path() -> PathBuf {
    crate::HorizonHome::resolve().root().join("upkeep").join("status.json")
}

/// What the person asked the board for; the app carries it out after drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open this pull request page in a browser panel beside the board.
    Open(String),
}

/// Panel content of the Upkeep board.
pub struct UpkeepBoard {
    path: PathBuf,
    /// The last valid status; it stays while a newer document is broken or gone.
    pub status: Option<Arc<Status>>,
    /// Why the document could not be read, the last time it changed.
    pub error: Option<String>,
    pub request: Option<Request>,
    latest: Option<reader::Latest>,
    alive: Arc<AtomicBool>,
}

impl UpkeepBoard {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(default_status_path),
            status: None,
            error: None,
            request: None,
            latest: None,
            alive: Arc::new(AtomicBool::new(true)),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes what the reader thread found since the last call, starting the thread on
    /// the first call. Never touches the file itself. Returns true when anything changed.
    pub fn poll(&mut self) -> bool {
        if self.latest.is_none() {
            self.latest = reader::spawn(self.path.clone(), self.alive.clone());
            if self.latest.is_none() {
                self.error = Some("The Upkeep status reader could not start.".into());
                return true;
            }
        }
        let reading = self
            .latest
            .as_ref()
            .and_then(|latest| latest.lock().ok().and_then(|mut latest| latest.take()));
        match reading {
            None => false,
            Some(Ok(status)) => {
                self.status = Some(Arc::new(status));
                self.error = None;
                true
            }
            Some(Err(error)) => {
                self.error = Some(error);
                true
            }
        }
    }
}

impl Drop for UpkeepBoard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

/// `owner/name` in the characters GitHub allows, with no `.` or `..` segment.
fn plain_repository(name: &str) -> bool {
    let mut parts = name.split('/');
    let (Some(owner), Some(repository), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, repository].iter().all(|part| {
        !part.is_empty()
            && *part != "."
            && *part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    })
}

/// "14:38 UTC", as other Horizon times show.
#[must_use]
pub fn clock(at: OffsetDateTime) -> String {
    let utc = at.to_offset(time::UtcOffset::UTC);
    format!("{:02}:{:02} UTC", utc.hour(), utc.minute())
}

/// "in 23 min", "3 h ago": how far `at` is from `now`, rounded to the useful unit.
#[must_use]
pub fn relative(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (at - now).whole_seconds();
    let span = seconds.unsigned_abs();
    let amount = if span < 60 {
        return if seconds >= 0 { "now".into() } else { "just now".into() };
    } else if span < 3600 {
        format!("{} min", span / 60)
    } else if span < 86_400 {
        format!("{} h", span / 3600)
    } else {
        format!("{} d", span / 86_400)
    };
    if seconds >= 0 {
        format!("in {amount}")
    } else {
        format!("{amount} ago")
    }
}

fn rfc3339<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<OffsetDateTime, D::Error> {
    let text = String::deserialize(deserializer)?;
    OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)
}

/// A value of the wrong type is the field's default, so one odd field does not hide the
/// whole status.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// An RFC 3339 time, or `None` for anything else.
fn lenient_time<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().and_then(parse_time))
}

/// The entries that parse; the others are left out.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(entries) = value else {
        return Ok(Vec::new());
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| T::deserialize(entry).ok())
        .collect())
}

/// Parses an RFC 3339 time, for tests and fixtures.
#[must_use]
pub fn parse_time(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_status_document_and_ignores_unknown_fields() {
        let text = r#"{"version":1,"agent":"codex","extra":true,
            "schedule":{"every_minutes":60,"next_run_at":"2026-10-13T15:00:00Z"},
            "pull_requests":[{"repository":"o/r","number":7,"title":"Bump a","url":"https://github.com/o/r/pull/7","state":"ready","checks":"passed"}]}"#;
        let status: Status = serde_json::from_str(text).unwrap();
        assert_eq!(status.agent, "codex");
        assert_eq!(status.pull_requests[0].state, PrState::Ready);
        assert_eq!(
            status.pull_requests[0].github_url().as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
    }

    #[test]
    fn opens_only_the_exact_pull_request_page() {
        let mut pr: PullRequest = serde_json::from_str(
            r#"{"repository":"o/r","number":7,"title":"t","url":"https://evil.example/o/r/pull/7","state":"ready"}"#,
        )
        .unwrap();
        assert_eq!(pr.github_url(), None);
        pr.url = "https://github.com/o/r/pull/8".into();
        assert_eq!(pr.github_url(), None);
        for repository in ["o/r/../../x", "o/..", "o", "o/r?x=1", "o/r#x"] {
            pr.repository = repository.into();
            pr.url = format!("https://github.com/{repository}/pull/7");
            pr.number = 7;
            assert_eq!(pr.github_url(), None, "{repository}");
        }
    }

    #[test]
    fn unknown_states_parse_as_other() {
        let status: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T10:00:00Z","outcome":"cancelled"}],
                "pull_requests":[{"repository":"o/r","number":1,"title":"t","url":"","state":"closed","checks":"stale"}]}"#,
        )
        .unwrap();
        assert_eq!(status.runs[0].outcome, RunOutcome::Other);
        assert_eq!(status.pull_requests[0].state, PrState::Other);
        assert_eq!(status.pull_requests[0].checks, Checks::None);
    }

    #[test]
    fn odd_fields_fall_back_and_broken_entries_are_left_out() {
        let status: Status = serde_json::from_str(
            r#"{"agent":7,"schedule":{"every_minutes":-1,"next_run_at":"","paused":"yes"},
                "runs":[{"started_at":"2026-10-13T10:00:00Z","merged":5000000000},{"merged":1}],
                "pull_requests":[{"repository":"o/r","number":1,"checks":null},{"title":"no repository"}],
                "repositories":["a/b",7],
                "activity":"none"}"#,
        )
        .unwrap();
        assert_eq!(status.agent, "");
        assert_eq!(status.schedule.every_minutes, 0);
        assert!(status.schedule.next_run_at.is_none() && !status.schedule.paused);
        assert_eq!(status.runs.len(), 1);
        assert_eq!(status.runs[0].merged, 0);
        assert_eq!(status.pull_requests.len(), 1);
        assert_eq!(status.pull_requests[0].state, PrState::Other);
        assert_eq!(status.pull_requests[0].checks, Checks::None);
        assert!(status.activity.is_empty());
        assert_eq!(status.repositories, ["a/b"]);
    }

    #[test]
    fn the_board_keeps_the_last_status_with_the_reason_it_could_not_read_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        let mut board = UpkeepBoard::new(Some(path.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while board.status.is_none() && std::time::Instant::now() < deadline {
            board.poll();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(board.status.as_ref().map(|status| status.agent.as_str()), Some("codex"));
        std::fs::remove_file(&path).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while board.error.is_none() && std::time::Instant::now() < deadline {
            board.poll();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(board.error.as_deref(), Some("The Upkeep status file is gone."));
        assert!(board.status.is_some());
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = parse_time("2026-10-13T14:37:00Z").unwrap();
        assert_eq!(relative(parse_time("2026-10-13T15:00:00Z").unwrap(), now), "in 23 min");
        assert_eq!(relative(parse_time("2026-10-13T11:37:00Z").unwrap(), now), "3 h ago");
        assert_eq!(relative(parse_time("2026-10-13T14:36:30Z").unwrap(), now), "just now");
    }
}
