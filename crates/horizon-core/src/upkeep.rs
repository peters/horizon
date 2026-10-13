//! Upkeep: one agent on a cloud worker that keeps dependency pull requests moving.
//!
//! The worker's agent writes a status document after every change; the Upkeep board
//! panel only reads it. The document is external input: it is size-limited, parsed
//! into a fixed schema and never run.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The time crate, so panels that show these times need no dependency of their own.
pub use ::time;

/// Largest status document the board reads.
const STATUS_LIMIT: u64 = 4 * 1024 * 1024;
/// How often the board looks for a newer document.
const POLL_EVERY: Duration = Duration::from_secs(2);

/// What the agent reports. Unknown fields are ignored so the worker can add some.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub worker: Worker,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub repositories: Vec<String>,
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequest>,
    #[serde(default)]
    pub activity: Vec<Activity>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Worker {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Schedule {
    #[serde(default)]
    pub every_minutes: u32,
    #[serde(default, deserialize_with = "rfc3339_option")]
    pub next_run_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    #[default]
    Done,
    Running,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Run {
    #[serde(deserialize_with = "rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(default, deserialize_with = "rfc3339_option")]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub outcome: RunOutcome,
    #[serde(default)]
    pub merged: u32,
}

/// Where a pull request stands, as the agent sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
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
}

impl PrState {
    pub const ORDER: [Self; 6] = [
        Self::Blocked,
        Self::Fixing,
        Self::NeedsReview,
        Self::WaitingChecks,
        Self::Ready,
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
    #[default]
    None,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequest {
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub checks: Checks,
    pub state: PrState,
    #[serde(default)]
    pub note: String,
    #[serde(default, deserialize_with = "rfc3339_option")]
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
    pub status: Option<Status>,
    pub error: Option<String>,
    pub request: Option<Request>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
}

impl UpkeepBoard {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(default_status_path),
            status: None,
            error: None,
            request: None,
            modified: None,
            checked: None,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the document again when it changed. Cheap between changes: one `stat`
    /// at most every two seconds. Returns true when the status changed.
    pub fn poll(&mut self) -> bool {
        if self.checked.is_some_and(|checked| checked.elapsed() < POLL_EVERY) {
            return false;
        }
        self.checked = Some(Instant::now());
        let modified = std::fs::metadata(&self.path).and_then(|meta| meta.modified()).ok();
        if modified.is_some() && modified == self.modified {
            return false;
        }
        self.modified = modified;
        match read_status(&self.path) {
            Ok(status) => {
                self.status = Some(status);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
}

fn read_status(path: &Path) -> Result<Status, String> {
    let meta = std::fs::metadata(path).map_err(|_| "Upkeep has not reported yet.".to_string())?;
    if meta.len() > STATUS_LIMIT {
        return Err("The Upkeep status is too large to read.".into());
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("The Upkeep status is not valid: {error}"))
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

fn rfc3339_option<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    text.map(|text| OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom))
        .transpose()
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
    fn a_missing_or_broken_document_is_a_message_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut board = UpkeepBoard::new(Some(path.clone()));
        assert!(board.poll());
        assert_eq!(board.error.as_deref(), Some("Upkeep has not reported yet."));
        std::fs::write(&path, "{").unwrap();
        board.checked = None;
        assert!(board.poll());
        assert!(
            board
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("The Upkeep status is not valid"))
        );
        assert!(board.status.is_none());
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = parse_time("2026-10-13T14:37:00Z").unwrap();
        assert_eq!(relative(parse_time("2026-10-13T15:00:00Z").unwrap(), now), "in 23 min");
        assert_eq!(relative(parse_time("2026-10-13T11:37:00Z").unwrap(), now), "3 h ago");
        assert_eq!(relative(parse_time("2026-10-13T14:36:30Z").unwrap(), now), "just now");
    }
}
