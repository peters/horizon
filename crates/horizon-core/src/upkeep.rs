//! Upkeep: one agent on a cloud worker that keeps dependency pull requests moving.
//!
//! The worker's agent writes a status document after every change; the Upkeep board
//! panel only reads it. The document is external input: it is read off the UI thread,
//! size-limited, parsed into a fixed schema with bounded lists and texts, and never run.
//! Unknown fields are ignored, and unknown states show as "Other".
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

mod agent_change;
#[cfg(feature = "cloud-workspaces")]
mod checkout;
#[cfg(feature = "cloud-workspaces")]
mod discover;
#[cfg(feature = "cloud-workspaces")]
pub mod native;
mod reader;
#[cfg(feature = "cloud-workspaces")]
pub mod worker;

pub use agent_change::{AgentChange, AgentChangeKind, record as record_agent_change};
#[cfg(feature = "cloud-workspaces")]
pub use checkout::checkout;
#[cfg(feature = "cloud-workspaces")]
pub use discover::{Catalog, CatalogRepository, discover};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
        self.has_page().then(|| self.url.clone())
    }

    /// Whether [`Self::github_url`] gives a page, checked without allocating.
    #[must_use]
    pub fn has_page(&self) -> bool {
        plain_repository(&self.repository)
            && self
                .url
                .strip_prefix("https://github.com/")
                .and_then(|rest| rest.strip_prefix(self.repository.as_str()))
                .and_then(|rest| rest.strip_prefix("/pull/"))
                .is_some_and(|number| {
                    // Exactly the decimal form: no sign, no leading zero, nothing after it.
                    number.bytes().all(|b| b.is_ascii_digit())
                        && !(number.len() > 1 && number.starts_with('0'))
                        && number.parse::<u64>().ok() == Some(self.number)
                })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Activity {
    #[serde(deserialize_with = "rfc3339")]
    pub at: OffsetDateTime,
    pub text: String,
}

/// The default instructions every Upkeep agent starts from.
pub const DEFAULT_INSTRUCTIONS: &str = "\
Keep dependency pull requests moving in the selected repositories.

The person who set up this Upkeep explicitly asks you to squash-merge the pull requests \
that the listed bots open in the listed repositories, once every gate passes. This is the \
explicit merge request that a repository's AGENTS.md may require. When a merge deploys to \
production or publishes a package by itself, that is authorized too, but only as the \
repository's AGENTS.md and the shared procedures it links describe it: keep their order, \
verify each deployment or package the way they say before the next merge, and stop when one \
cannot be verified. Never start a release or a production workflow by hand.

1. Read each repository's AGENTS.md and follow it. Shared rules in a linked \
company repository apply too.
2. Review the manifest and lockfile diff. Rebase only a branch that conflicts, or one the \
repository requires to be up to date; a branch that is merely behind needs no rebase.
3. Fix checks that the update broke on the pull request branch. Never weaken a test.
4. Squash-merge with `gh pr merge --squash --match-head-commit <head>` only when every check, \
review and required smoke passes for that exact head. \
When a decision only a person can make blocks a merge, leave the pull request, mark it \
blocked and say why.
   - A smoke test this worker cannot run (a GPU, a physical device or a test rig) is covered \
by CI for a pure dependency update when the required CI jobs pass for the exact head. A native \
desktop qualification runs on a test cloud instead, as described below.
   - When a gate needs a preview deployment that CI skips for bot pull requests, open one \
separate pull request in that repository that enables previews for bot pull requests, for a \
person to review. Reuse an open one. The update stays blocked until it merges.
   - A repository in a required rollout order that has no open pull request from these bots \
counts as done for that order.
5. After every change, update the Upkeep status file.";

/// Where the board reads the status document when its panel names none.
#[must_use]
pub fn default_status_path() -> PathBuf {
    crate::HorizonHome::resolve().root().join("upkeep").join("status.json")
}

/// Reads the status document at `path` once, with the board's limits. It blocks: call it
/// off the UI thread.
/// # Errors
/// A document that cannot be read or is not valid.
pub fn read_status(path: &Path) -> Result<Status, String> {
    reader::read_status(path)
}

/// What the person asked the board for; the app carries it out after drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open this pull request page in a browser panel beside the board.
    Open(String),
    /// Start a run now instead of at the next planned time.
    RunNow,
    /// Stop (`true`) or start again (`false`) the planned runs.
    Pause(bool),
    /// Edit the repositories of this Upkeep.
    Repositories,
}

/// Panel content of the Upkeep board.
pub struct UpkeepBoard {
    path: PathBuf,
    /// The last valid status; it stays while a newer document is broken or gone.
    pub status: Option<Arc<Status>>,
    /// Why the document could not be read, the last time it changed.
    pub error: Option<String>,
    pub request: Option<Request>,
    /// The browser panel this board opened for pull requests, while it exists.
    pub browser: Option<crate::PanelId>,
    /// The Upkeep cloud whose status this is, once the app knows it: only such a board
    /// offers Run now, Pause and Repositories.
    pub cloud: Option<String>,
    /// Run now was pressed: the start of the newest run the status showed then, and when it
    /// was pressed by the monotonic clock, until the status shows a newer run.
    pub run_requested: Option<(Option<OffsetDateTime>, std::time::Instant)>,
    /// What agents changed through `cloud_upkeep`, newest first; the app fills it in.
    pub agent_changes: Vec<AgentChange>,
    latest: Option<reader::Latest>,
    /// The reader thread could not start; the board does not try again.
    reader_failed: bool,
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
            browser: None,
            cloud: None,
            run_requested: None,
            agent_changes: Vec::new(),
            latest: None,
            reader_failed: false,
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
            if self.reader_failed {
                return false;
            }
            self.latest = reader::spawn(self.path.clone(), self.alive.clone());
            if self.latest.is_none() {
                self.reader_failed = true;
                self.error = Some("The Upkeep status reader could not start.".into());
                return true;
            }
        }
        let Some((status, error)) = self.latest.as_ref().and_then(|latest| {
            let mut pending = latest.lock().ok()?;
            Some((pending.status.take(), pending.error.take()))
        }) else {
            return false;
        };
        let changed = status.is_some() || error.is_some();
        if let Some(status) = status {
            self.status = Some(Arc::new(status));
        }
        match error {
            Some(reader::ErrorChange::Cleared) => self.error = None,
            Some(reader::ErrorChange::Set(error)) => self.error = Some(error),
            None => {}
        }
        changed
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
    parse_time(&text).ok_or_else(|| serde::de::Error::custom("not an RFC 3339 time in UTC range"))
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

/// The entries that parse; the others are left out. A value that is not a list is an empty
/// list. Every entry is read, but the kept ones are pruned to the most important whenever
/// they reach twice their limit, so the order of the document decides nothing and a
/// document of many tiny entries stays bounded in memory.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + reader::Bounded,
{
    struct Entries<T>(std::marker::PhantomData<T>);

    impl<'de, T: serde::de::DeserializeOwned + reader::Bounded> serde::de::Visitor<'de> for Entries<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a list")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = seq.next_element::<serde_json::Value>()? {
                if let Ok(entry) = T::deserialize(entry) {
                    entries.push(entry);
                    if entries.len() >= 2 * T::KEEP {
                        T::prune(&mut entries);
                    }
                }
            }
            Ok(entries)
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
            while map
                .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                .is_some()
            {}
            Ok(Vec::new())
        }

        fn visit_bool<E>(self, _: bool) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }

        fn visit_i64<E>(self, _: i64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }

        fn visit_u64<E>(self, _: u64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }

        fn visit_f64<E>(self, _: f64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }

        fn visit_str<E>(self, _: &str) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }

        fn visit_unit<E>(self) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(Entries(std::marker::PhantomData))
}

/// Parses an RFC 3339 time and turns it into UTC. A time whose UTC date is out of range
/// is `None`, so no later conversion can panic on it.
#[must_use]
pub fn parse_time(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339)
        .ok()?
        .checked_to_offset(time::UtcOffset::UTC)
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
        for url in [
            "https://github.com/o/r/pull/07",
            "https://github.com/o/r/pull/+7",
            "https://github.com/o/r/pull/7/files",
        ] {
            pr.repository = "o/r".into();
            pr.number = 7;
            pr.url = url.into();
            assert!(!pr.has_page(), "{url}");
        }
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
    fn a_time_out_of_range_in_utc_is_refused_not_a_panic() {
        assert!(parse_time("9999-12-31T23:59:59-23:59").is_none());
        let status: Status = serde_json::from_str(
            r#"{"schedule":{"next_run_at":"9999-12-31T23:59:59-23:59"},
                "activity":[{"at":"9999-12-31T23:59:59-23:59","text":"x"},{"at":"2026-10-13T12:00:00+02:00","text":"y"}]}"#,
        )
        .unwrap();
        assert!(status.schedule.next_run_at.is_none());
        assert_eq!(status.activity.len(), 1);
        assert_eq!(clock(status.activity[0].at), "10:00 UTC");
    }

    #[test]
    fn a_long_list_keeps_what_matters_whatever_its_order() {
        let mut entries: Vec<String> = (0..10_050)
            .map(|n| {
                format!(r#"{{"repository":"o/r","number":{n},"state":"merged","updated_at":"2026-10-13T10:00:00Z"}}"#)
            })
            .collect();
        entries.push(
            r#"{"repository":"o/r","number":99999,"state":"blocked","updated_at":"2026-01-01T00:00:00Z"}"#.into(),
        );
        let status: Status = serde_json::from_str(&format!(r#"{{"pull_requests":[{}]}}"#, entries.join(","))).unwrap();
        assert!(status.pull_requests.len() <= 2 * 500);
        assert!(
            status.pull_requests.iter().any(|pr| pr.number == 99999),
            "the blocked one at the end stays"
        );
        let names = vec![r#""o/r""#; 3000].join(",");
        let status: Status =
            serde_json::from_str(&format!(r#"{{"repositories":[{names}],"activity":{{"a":1}}}}"#)).unwrap();
        assert!(status.repositories.len() < 2000);
        assert!(status.activity.is_empty());
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
