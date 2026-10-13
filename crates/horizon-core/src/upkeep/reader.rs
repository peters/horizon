//! Reads the status document off the UI thread. The document comes from an agent on a
//! cloud worker, so it is untrusted: its size, its lists and its texts are bounded
//! before the board draws them.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use super::Status;

/// Largest status document read.
const STATUS_LIMIT: u64 = 4 * 1024 * 1024;
/// How often the reader looks for a newer document.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// The most pull requests, runs, activity lines and repositories a board keeps.
const MAX_PULL_REQUESTS: usize = 500;
const MAX_RUNS: usize = 100;
const MAX_ACTIVITY: usize = 100;
const MAX_REPOSITORIES: usize = 1000;
/// The longest text a board keeps, in characters.
const MAX_TEXT: usize = 300;

/// What one look at the document found.
pub(super) type Reading = Result<Status, String>;

/// Looks at the document when asked; [`spawn`] asks every two seconds on its own thread.
pub(super) struct Reader {
    path: PathBuf,
    /// Whether the document was looked at before.
    looked: bool,
    /// The modification time at the last look, `None` for a missing file.
    modified: Option<SystemTime>,
    /// Whether a valid status was read before.
    read_once: bool,
}

impl Reader {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            looked: false,
            modified: None,
            read_once: false,
        }
    }

    /// A new reading when the document changed, appeared or went since the last look.
    pub(super) fn check(&mut self) -> Option<Reading> {
        let modified = std::fs::metadata(&self.path).and_then(|meta| meta.modified()).ok();
        if self.looked && self.modified == modified {
            return None;
        }
        self.looked = true;
        self.modified = modified;
        if modified.is_none() {
            return Some(Err(if self.read_once {
                "The Upkeep status file is gone.".into()
            } else {
                "Upkeep has not reported yet.".into()
            }));
        }
        let reading = read_status(&self.path);
        self.read_once |= reading.is_ok();
        Some(reading)
    }
}

/// Starts a thread that sends a reading whenever the document changes. It ends when
/// `alive` turns false or the receiver is gone.
pub(super) fn spawn(path: PathBuf, alive: Arc<AtomicBool>) -> Option<Receiver<Reading>> {
    let (sender, receiver) = mpsc::channel();
    let mut reader = Reader::new(path);
    std::thread::Builder::new()
        .name("upkeep-status".into())
        .spawn(move || {
            while alive.load(Ordering::Relaxed) {
                if let Some(reading) = reader.check()
                    && sender.send(reading).is_err()
                {
                    return;
                }
                std::thread::sleep(CHECK_EVERY);
            }
        })
        .ok()?;
    Some(receiver)
}

fn read_status(path: &Path) -> Reading {
    let file = std::fs::File::open(path).map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    // The limit holds even when the file grows while it is read.
    let mut text = String::new();
    file.take(STATUS_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("The Upkeep status cannot be read: {error}"))?;
    if text.len() as u64 > STATUS_LIMIT {
        return Err("The Upkeep status is too large to read.".into());
    }
    let mut status: Status =
        serde_json::from_str(&text).map_err(|error| format!("The Upkeep status is not valid: {error}"))?;
    bound(&mut status);
    Ok(status)
}

/// Keeps the newest entries of each list and clips each text, so a large document
/// cannot make every frame slow.
fn bound(status: &mut Status) {
    keep_last(&mut status.runs, MAX_RUNS);
    keep_last(&mut status.activity, MAX_ACTIVITY);
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
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        assert!(
            reader
                .check()
                .unwrap()
                .unwrap_err()
                .starts_with("The Upkeep status is not valid")
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(reader.check().unwrap().unwrap_err(), "The Upkeep status file is gone.");
    }

    #[test]
    fn a_large_document_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let long = "x".repeat(1_000);
        let pull_requests: Vec<serde_json::Value> = (0..2000)
            .map(|n| serde_json::json!({"repository": "o/r", "number": n, "title": long, "url": "", "state": "ready"}))
            .collect();
        let runs: Vec<serde_json::Value> = (0..500)
            .map(|n| serde_json::json!({"started_at": "2026-10-13T10:00:00Z", "merged": n}))
            .collect();
        let document = serde_json::json!({"agent": long, "pull_requests": pull_requests, "runs": runs});
        std::fs::write(&path, document.to_string()).unwrap();
        let status = read_status(&path).unwrap();
        assert_eq!(status.pull_requests.len(), MAX_PULL_REQUESTS);
        assert_eq!(status.pull_requests[0].title.chars().count(), MAX_TEXT + 1);
        assert_eq!(status.agent.chars().count(), MAX_TEXT + 1);
        assert_eq!(status.runs.len(), MAX_RUNS);
        assert_eq!(status.runs[0].merged, 400, "the newest runs stay");
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
    fn the_thread_sends_the_first_reading_and_stops_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        std::fs::write(&path, r#"{"agent":"codex"}"#).unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let receiver = spawn(path, alive.clone()).unwrap();
        let reading = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(reading.unwrap().agent, "codex");
        alive.store(false, Ordering::Relaxed);
        assert!(
            receiver.recv_timeout(Duration::from_secs(5)).is_err(),
            "the thread ends"
        );
    }
}
