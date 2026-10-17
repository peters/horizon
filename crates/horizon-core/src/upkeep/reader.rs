//! Reads the status document off the UI thread. The document comes from an agent on a
//! cloud worker, so it is untrusted: its size, its lists and its texts are bounded
//! before the board draws them.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{PrState, Status};

/// Largest status document read.
const STATUS_LIMIT: u64 = 4 * 1024 * 1024;
/// How often the reader looks for a newer document.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// The most pull requests, runs, activity lines and repositories a board keeps.
const MAX_PULL_REQUESTS: usize = 500;
/// Runs: one a minute for a day at most.
const MAX_RUNS: usize = 1500;
const MAX_ACTIVITY: usize = 100;
const MAX_REPOSITORIES: usize = 1000;
/// The longest text a board keeps, in characters.
const MAX_TEXT: usize = 300;

/// What one look at the document found.
pub(super) type Reading = Result<Status, String>;

/// Looks at the document when asked; [`spawn`] asks every two seconds on its own thread.
pub(super) struct Reader {
    path: PathBuf,
    /// A fingerprint of the bytes last parsed. The file's time and length are not enough: a
    /// copy from the worker can keep both and still change.
    last_hash: Option<u64>,
    /// Whether the file was missing at the last look.
    missing: bool,
    /// The last error sent, so a read that keeps failing the same way is sent once.
    last_error: Option<String>,
    /// Whether a valid status was read before.
    read_once: bool,
    /// [`super::PACKAGE_ACCESS_SKIPPED`] beside the document, and whether it was there at the
    /// last look.
    skip_mark: PathBuf,
    skipped: Option<bool>,
}

impl Reader {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            skip_mark: path.with_file_name(super::PACKAGE_ACCESS_SKIPPED),
            skipped: None,
            path,
            last_hash: None,
            missing: false,
            last_error: None,
            read_once: false,
        }
    }

    /// A new reading when the document changed, appeared or went since the last look.
    /// The bytes are read at every look and compared by fingerprint, so any change counts;
    /// a failure is sent once until the file changes again.
    pub(super) fn check(&mut self) -> Option<Reading> {
        // Only a file that is not there is missing; any other failure is reported as it is.
        let missing =
            std::fs::symlink_metadata(&self.path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        let reading = if missing {
            if self.missing {
                return None;
            }
            self.missing = true;
            self.last_hash = None;
            Err(if self.read_once {
                "The Upkeep status file is gone.".to_string()
            } else {
                "Upkeep has not reported yet.".to_string()
            })
        } else {
            self.missing = false;
            match read_bytes(&self.path) {
                Err(error) => {
                    // The next readable content counts as new, even the last valid one.
                    self.last_hash = None;
                    Err(error)
                }
                Ok(bytes) => {
                    let hash = fingerprint(&bytes);
                    if self.last_hash == Some(hash) {
                        return None;
                    }
                    self.last_hash = Some(hash);
                    parse_status(&bytes)
                }
            }
        };
        match reading {
            Ok(status) => {
                self.read_once = true;
                self.last_error = None;
                Some(Ok(status))
            }
            Err(error) if self.last_error.as_ref() == Some(&error) => None,
            Err(error) => {
                self.last_error = Some(error.clone());
                Some(Err(error))
            }
        }
    }
}

impl Reader {
    /// Whether the person skipped the package read access, when that changed since the last
    /// look. It reads the mark as the deployment does, so the board offers what it would ask.
    pub(super) fn check_skipped(&mut self) -> Option<bool> {
        let skipped = self.skip_mark.exists();
        (self.skipped != Some(skipped)).then(|| {
            self.skipped = Some(skipped);
            skipped
        })
    }
}

/// The newest reading the thread found and the board has not taken yet. A newer reading
/// replaces an older one, so a board that is not drawn holds at most one.
pub(super) type Latest = Arc<Mutex<Pending>>;

/// What the thread found and the board has not taken yet: the newest valid status and
/// the newest error. A later error does not hide a valid status the board has not taken.
#[derive(Default)]
pub(super) struct Pending {
    pub(super) status: Option<Status>,
    pub(super) error: Option<ErrorChange>,
    /// Whether the package read access is skipped, when that changed.
    pub(super) package_skipped: Option<bool>,
}

/// How the board's error line changes.
pub(super) enum ErrorChange {
    /// A valid status ended an earlier error.
    Cleared,
    Set(String),
}

impl Pending {
    fn put(&mut self, reading: Reading) {
        match reading {
            Ok(status) => {
                self.status = Some(status);
                self.error = Some(ErrorChange::Cleared);
            }
            Err(error) => self.error = Some(ErrorChange::Set(error)),
        }
    }
}

/// Starts a thread that puts a reading into the returned slot whenever the document
/// changes. It ends when `alive` turns false or the board dropped the slot.
pub(super) fn spawn(path: PathBuf, alive: Arc<AtomicBool>) -> Option<Latest> {
    let latest = Latest::default();
    let slot = Arc::downgrade(&latest);
    let mut reader = Reader::new(path);
    std::thread::Builder::new()
        .name("upkeep-status".into())
        .spawn(move || {
            while alive.load(Ordering::Relaxed) {
                let Some(slot) = slot.upgrade() else {
                    return;
                };
                let (reading, skipped) = (reader.check(), reader.check_skipped());
                if (reading.is_some() || skipped.is_some())
                    && let Ok(mut pending) = slot.lock()
                {
                    if let Some(reading) = reading {
                        pending.put(reading);
                    }
                    if skipped.is_some() {
                        pending.package_skipped = skipped;
                    }
                }
                drop(slot);
                std::thread::sleep(CHECK_EVERY);
            }
        })
        .ok()?;
    Some(latest)
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    // The limit holds even when the file grows while it is read.
    let mut bytes = Vec::new();
    file.take(STATUS_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    if bytes.len() as u64 > STATUS_LIMIT {
        return Err("The Upkeep status is too large to read.".into());
    }
    Ok(bytes)
}

fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn parse_status(bytes: &[u8]) -> Reading {
    let mut status: Status = serde_json::from_slice(bytes).map_err(|error| {
        // The parser quotes the document; a quote of 4 MiB must not reach the board.
        let mut message = format!("The Upkeep status is not valid: {error}");
        clip(&mut message);
        message
    })?;
    bound(&mut status);
    Ok(status)
}

pub(super) fn read_status(path: &Path) -> Reading {
    parse_status(&read_bytes(path)?)
}

/// A list of the status that keeps a bounded selection: the entries a person needs most.
pub(super) trait Bounded: Sized {
    /// The most entries kept.
    const KEEP: usize;
    /// Orders the entries by importance and keeps the first [`Self::KEEP`].
    fn prune(list: &mut Vec<Self>);
}

impl Bounded for super::Run {
    const KEEP: usize = MAX_RUNS;

    /// The newest runs; a run dated far ahead cannot push the real ones out.
    fn prune(list: &mut Vec<Self>) {
        let horizon = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        list.retain(|run| run.started_at <= horizon);
        list.sort_by_key(|run| run.started_at);
        keep_last(list, Self::KEEP);
    }
}

impl Bounded for super::Activity {
    const KEEP: usize = MAX_ACTIVITY;

    fn prune(list: &mut Vec<Self>) {
        let horizon = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        list.retain(|entry| entry.at <= horizon);
        list.sort_by_key(|entry| entry.at);
        keep_last(list, Self::KEEP);
    }
}

impl Bounded for super::PullRequest {
    const KEEP: usize = MAX_PULL_REQUESTS;

    /// Open pull requests first, newest first, so a person never misses one for a merged one.
    /// A pull request listed twice keeps its most important entry.
    fn prune(list: &mut Vec<Self>) {
        list.sort_by_key(|pr| (pr.state == PrState::Merged, std::cmp::Reverse(pr.updated_at)));
        let mut seen = std::collections::HashSet::new();
        list.retain(|pr| seen.insert((pr.repository.to_ascii_lowercase(), pr.number)));
        list.truncate(Self::KEEP);
    }
}

impl Bounded for String {
    const KEEP: usize = MAX_REPOSITORIES;

    fn prune(list: &mut Vec<Self>) {
        list.truncate(Self::KEEP);
    }
}

/// Puts runs and activity in time order, keeps the most important entries of each list
/// and clips each text, so a large document cannot make every frame slow.
fn bound(status: &mut Status) {
    super::Run::prune(&mut status.runs);
    super::Activity::prune(&mut status.activity);
    super::PullRequest::prune(&mut status.pull_requests);
    String::prune(&mut status.repositories);
    for text in [
        &mut status.agent,
        &mut status.worker.name,
        &mut status.worker.location,
        &mut status.worker.state,
    ] {
        clip(text);
    }
    for repository in &mut status.repositories {
        clip(repository);
    }
    for pr in &mut status.pull_requests {
        for text in [
            &mut pr.repository,
            &mut pr.title,
            &mut pr.url,
            &mut pr.author,
            &mut pr.note,
        ] {
            clip(text);
        }
    }
    for entry in &mut status.activity {
        clip(&mut entry.text);
    }
}

fn keep_last<T>(list: &mut Vec<T>, limit: usize) {
    let extra = list.len().saturating_sub(limit);
    list.drain(..extra);
}

/// Cuts a text longer than `MAX_TEXT` characters to `MAX_TEXT` with an ellipsis at the end.
fn clip(text: &mut String) {
    if text.chars().nth(MAX_TEXT).is_some()
        && let Some((cut, _)) = text.char_indices().nth(MAX_TEXT - 1)
    {
        text.truncate(cut);
        text.push('…');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skip_mark_beside_the_status_is_reported_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut reader = Reader::new(dir.path().join("status.json"));
        assert_eq!(reader.check_skipped(), Some(false), "the first look reports the state");
        assert_eq!(reader.check_skipped(), None);
        let mark = dir.path().join(crate::upkeep::PACKAGE_ACCESS_SKIPPED);
        std::fs::write(&mark, "skipped\n").unwrap();
        assert_eq!(reader.check_skipped(), Some(true));
        assert_eq!(reader.check_skipped(), None);
        std::fs::remove_file(&mark).unwrap();
        assert_eq!(reader.check_skipped(), Some(false));
    }

    #[test]
    fn reports_each_change_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut reader = Reader::new(path.clone());
        assert_eq!(reader.check().unwrap().unwrap_err(), "Upkeep has not reported yet.");
        assert!(reader.check().is_none(), "a file that is still missing is not news");
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        assert_eq!(reader.check().unwrap().unwrap().agent, "codex");
        assert!(reader.check().is_none());
        std::fs::write(&path, "{").unwrap();
        assert!(
            reader
                .check()
                .unwrap()
                .unwrap_err()
                .starts_with("The Upkeep status is not valid")
        );
        assert!(reader.check().is_none(), "the same failure is sent once");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(reader.check().unwrap().unwrap_err(), "The Upkeep status file is gone.");
    }

    #[test]
    fn a_failed_read_is_tried_again_even_when_the_file_looks_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut reader = Reader::new(path.clone());
        // The worker wrote part of the document; the rest lands with the same time and length.
        std::fs::write(&path, r#"{"agent":"cod"#).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(reader.check().unwrap().is_err());
        std::fs::write(&path, r#"{"agent":"c"}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(reader.check().unwrap().unwrap().agent, "c");
    }

    #[test]
    fn a_change_that_keeps_the_time_and_length_is_still_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut reader = Reader::new(path.clone());
        std::fs::write(&path, r#"{"agent":"aaaa"}"#).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(reader.check().unwrap().unwrap().agent, "aaaa");
        std::fs::write(&path, r#"{"agent":"bbbb"}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(reader.check().unwrap().unwrap().agent, "bbbb");
    }

    #[test]
    fn the_last_valid_content_after_a_read_error_clears_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut reader = Reader::new(path.clone());
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        assert!(reader.check().unwrap().is_ok());
        std::fs::write(&path, " ".repeat(usize::try_from(STATUS_LIMIT).unwrap() + 1)).unwrap();
        assert_eq!(
            reader.check().unwrap().unwrap_err(),
            "The Upkeep status is too large to read."
        );
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        assert_eq!(reader.check().unwrap().unwrap().agent, "codex");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_is_a_read_error_not_a_missing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("status.json"), "{}").unwrap();
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut reader = Reader::new(inner.join("status.json"));
        let reading = reader.check();
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Root reads anything; only an ordinary account sees the refusal.
        if let Some(Err(error)) = reading {
            assert!(error.starts_with("The Upkeep status cannot be read"), "{error}");
        }
    }

    #[test]
    fn a_huge_invalid_document_gives_a_short_error_and_duplicates_go() {
        let quoted = format!("\"{}\"", "x".repeat(1_000_000));
        let error = parse_status(quoted.as_bytes()).unwrap_err();
        assert!(
            error.chars().count() <= MAX_TEXT,
            "{} characters",
            error.chars().count()
        );
        let status = parse_status(
            br#"{"pull_requests":[{"repository":"o/r","number":7,"state":"merged"},{"repository":"O/R","number":7,"state":"blocked"}]}"#,
        )
        .unwrap();
        assert_eq!(status.pull_requests.len(), 1);
        assert_eq!(status.pull_requests[0].state, PrState::Blocked);
    }

    #[test]
    fn a_clipped_text_keeps_the_limit_and_a_text_at_the_limit_stays() {
        let mut long = "x".repeat(MAX_TEXT + 5);
        clip(&mut long);
        assert_eq!(long.chars().count(), MAX_TEXT);
        assert!(long.ends_with('…'));
        let mut exact = "y".repeat(MAX_TEXT);
        clip(&mut exact);
        assert_eq!(exact, "y".repeat(MAX_TEXT));
    }

    #[test]
    fn runs_and_activity_are_put_in_time_order_before_the_newest_are_kept() {
        let mut status: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T12:00:00Z","merged":2},{"started_at":"2026-10-13T10:00:00Z","merged":1}],
                "activity":[{"at":"2026-10-13T12:00:00Z","text":"b"},{"at":"2026-10-13T10:00:00Z","text":"a"}]}"#,
        )
        .unwrap();
        bound(&mut status);
        assert_eq!(status.runs.last().map(|run| run.merged), Some(2));
        assert_eq!(status.activity.last().map(|entry| entry.text.as_str()), Some("b"));
    }

    #[test]
    fn a_document_cut_inside_a_character_at_the_limit_is_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut text = " ".repeat(usize::try_from(STATUS_LIMIT).unwrap() - 1);
        text.push('é');
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read_status(&path).unwrap_err(),
            "The Upkeep status is too large to read."
        );
    }

    #[test]
    fn the_cut_keeps_open_pull_requests_before_merged_ones() {
        let mut pull_requests: Vec<serde_json::Value> = (0..MAX_PULL_REQUESTS + 20)
            .map(|n| serde_json::json!({"repository": "o/r", "number": n, "state": "merged", "updated_at": "2026-10-13T10:00:00Z"}))
            .collect();
        pull_requests.push(serde_json::json!({"repository": "o/r", "number": 9999, "state": "blocked", "updated_at": "2026-01-01T00:00:00Z"}));
        let mut status: Status = serde_json::from_value(serde_json::json!({"pull_requests": pull_requests})).unwrap();
        bound(&mut status);
        assert_eq!(status.pull_requests.len(), MAX_PULL_REQUESTS);
        assert_eq!(status.pull_requests[0].number, 9999);
    }

    #[test]
    fn a_large_document_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let long = "x".repeat(1_000);
        let pull_requests: Vec<serde_json::Value> = (0..2000)
            .map(|n| serde_json::json!({"repository": "o/r", "number": n, "title": long, "url": "", "state": "ready"}))
            .collect();
        let runs: Vec<serde_json::Value> = (0..2000)
            .map(|n| serde_json::json!({"started_at": "2026-10-13T10:00:00Z", "merged": n}))
            .collect();
        let document = serde_json::json!({"agent": long, "pull_requests": pull_requests, "runs": runs});
        std::fs::write(&path, document.to_string()).unwrap();
        let status = read_status(&path).unwrap();
        assert_eq!(status.pull_requests.len(), MAX_PULL_REQUESTS);
        assert_eq!(status.pull_requests[0].title.chars().count(), MAX_TEXT);
        assert_eq!(status.agent.chars().count(), MAX_TEXT);
        assert_eq!(status.runs.len(), MAX_RUNS);
        assert_eq!(status.runs[0].merged, 500, "the newest runs stay");
    }

    #[test]
    fn a_document_over_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        std::fs::write(&path, " ".repeat(usize::try_from(STATUS_LIMIT).unwrap() + 1)).unwrap();
        assert_eq!(
            read_status(&path).unwrap_err(),
            "The Upkeep status is too large to read."
        );
    }

    #[test]
    fn the_newest_reading_wins_but_an_untaken_status_stays_with_a_later_error() {
        let status = |agent: &str| Status {
            agent: agent.into(),
            ..Status::default()
        };
        let mut pending = Pending::default();
        pending.put(Ok(status("one")));
        pending.put(Ok(status("two")));
        pending.put(Err("broken".into()));
        assert_eq!(pending.status.take().map(|status| status.agent), Some("two".into()));
        assert!(matches!(pending.error.take(), Some(ErrorChange::Set(error)) if error == "broken"));
        pending.put(Ok(status("three")));
        assert!(matches!(pending.error.take(), Some(ErrorChange::Cleared)));
    }

    #[test]
    fn the_thread_sends_the_first_reading_and_stops_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let latest = spawn(path.clone(), alive.clone()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = latest.lock().unwrap().status.take() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "no first reading");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.agent, "codex");
        alive.store(false, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(2500));
        std::fs::write(&path, r#"{"agent":"after"}"#).unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        assert!(latest.lock().unwrap().status.is_none(), "the thread ended");
    }
}
