//! Reads the status document off the UI thread. The document comes from an agent on a
//! cloud worker, so it is untrusted: its size, its lists and its texts are bounded
//! before the board draws them.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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
    /// Whether `seen` holds a look. It is false before the first look and after a read
    /// that failed, so that read is tried again.
    settled: bool,
    /// The file's modification time and length at the last settled look, `None` when missing.
    seen: Option<(SystemTime, u64)>,
    /// The last error sent, so a read that keeps failing the same way is sent once.
    last_error: Option<String>,
    /// Whether a valid status was read before.
    read_once: bool,
}

impl Reader {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            settled: false,
            seen: None,
            last_error: None,
            read_once: false,
        }
    }

    /// A new reading when the document changed, appeared or went since the last look.
    /// A file is the same while its modification time and length are; a read that
    /// failed is tried again at the next look.
    pub(super) fn check(&mut self) -> Option<Reading> {
        let found = std::fs::metadata(&self.path)
            .ok()
            .map(|meta| (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()));
        if self.settled && self.seen == found {
            return None;
        }
        let reading = if found.is_none() {
            self.settled = true;
            Err(if self.read_once {
                "The Upkeep status file is gone.".to_string()
            } else {
                "Upkeep has not reported yet.".to_string()
            })
        } else {
            let reading = read_status(&self.path);
            self.settled = reading.is_ok();
            reading
        };
        self.seen = found;
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

/// The newest reading the thread found and the board has not taken yet. A newer reading
/// replaces an older one, so a board that is not drawn holds at most one.
pub(super) type Latest = Arc<Mutex<Pending>>;

/// What the thread found and the board has not taken yet: the newest valid status and
/// the newest error. A later error does not hide a valid status the board has not taken.
#[derive(Default)]
pub(super) struct Pending {
    pub(super) status: Option<Status>,
    pub(super) error: Option<ErrorChange>,
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
                if let Some(reading) = reader.check()
                    && let Ok(mut pending) = slot.lock()
                {
                    pending.put(reading);
                }
                drop(slot);
                std::thread::sleep(CHECK_EVERY);
            }
        })
        .ok()?;
    Some(latest)
}

fn read_status(path: &Path) -> Reading {
    let file = std::fs::File::open(path).map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    // The limit holds even when the file grows while it is read.
    let mut bytes = Vec::new();
    file.take(STATUS_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    if bytes.len() as u64 > STATUS_LIMIT {
        return Err("The Upkeep status is too large to read.".into());
    }
    let mut status: Status =
        serde_json::from_slice(&bytes).map_err(|error| format!("The Upkeep status is not valid: {error}"))?;
    bound(&mut status);
    Ok(status)
}

/// Puts runs and activity in time order, keeps the newest entries of each list and
/// clips each text, so a large document cannot make every frame slow.
fn bound(status: &mut Status) {
    // A run dated far ahead cannot push the real ones out.
    let horizon = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    status.runs.retain(|run| run.started_at <= horizon);
    status.runs.sort_by_key(|run| run.started_at);
    keep_last(&mut status.runs, MAX_RUNS);
    status.activity.retain(|entry| entry.at <= horizon);
    status.activity.sort_by_key(|entry| entry.at);
    keep_last(&mut status.activity, MAX_ACTIVITY);
    // Open pull requests first, so a person never misses one for a merged one.
    status
        .pull_requests
        .sort_by_key(|pr| (pr.state == PrState::Merged, std::cmp::Reverse(pr.updated_at)));
    status.pull_requests.truncate(MAX_PULL_REQUESTS);
    status.repositories.truncate(MAX_REPOSITORIES);
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

fn clip(text: &mut String) {
    if let Some((cut, _)) = text.char_indices().nth(MAX_TEXT) {
        text.truncate(cut);
        text.push('…');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(status.pull_requests[0].title.chars().count(), MAX_TEXT + 1);
        assert_eq!(status.agent.chars().count(), MAX_TEXT + 1);
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
