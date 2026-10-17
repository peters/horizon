//! The panel content of the Upkeep board: the status the reader thread found, and what the
//! person asked for.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use time::OffsetDateTime;

use super::{AgentChange, Request, Status, default_status_path, reader};

/// Beside the status of an Upkeep's cloud while the person skipped its package read access:
/// a reconnect does not ask again, and the board offers to.
pub const PACKAGE_ACCESS_SKIPPED: &str = "package-access-skipped";

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
    /// The person skipped the package read access of this board's Upkeep, as the reader
    /// thread last saw [`PACKAGE_ACCESS_SKIPPED`] beside the status.
    pub package_skipped: bool,
    /// Why the last Give package access did not start, until the next press.
    pub package_note: Option<String>,
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
            package_skipped: false,
            package_note: None,
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
        let Some((status, error, skipped)) = self.latest.as_ref().and_then(|latest| {
            let mut pending = latest.lock().ok()?;
            Some((
                pending.status.take(),
                pending.error.take(),
                pending.package_skipped.take(),
            ))
        }) else {
            return false;
        };
        let changed = status.is_some() || error.is_some() || skipped.is_some();
        if let Some(skipped) = skipped {
            self.package_skipped = skipped;
        }
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
