//! Durable session saves on one worker thread, so the UI thread never waits for a disk.
//!
//! The UI queues the newest board of a session; a later queue of the same session replaces
//! a board that is not written yet. The worker writes each board, its metadata and the
//! session index so that a crash cannot leave a session without its content: each file is
//! written to a temporary file, synced, renamed into place, and its folder is synced. A
//! read of a session that is queued or being written returns the queued board, so a reader
//! never sees an older one. A save that fails is kept and tried again later, while the
//! saves of other sessions go on.
use super::{ParkedPanel, SessionStore};
use crate::RuntimeState;
use crate::cloud_runtime::session_status::SessionStatus;
use crate::error::{Error, Result as StoreResult};
use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// How long a failed save waits before it is tried again; each failure doubles it.
const FIRST_RETRY: Duration = Duration::from_millis(250);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// An idle worker ends after this long; the next queue starts a new one.
const IDLE: Duration = Duration::from_secs(30);

/// The queue of one [`SessionStore`] and all its clones.
#[derive(Clone, Default)]
pub(super) struct Writer {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Boards that are not written yet, by session.
    queued: BTreeMap<String, Queued>,
    /// The board the worker writes now.
    writing: Option<(String, Queued)>,
    /// The newest generation written for each session.
    written: BTreeMap<String, u64>,
    next: u64,
    running: bool,
    /// The last failed save of each session, until a save of that session succeeds.
    failed: BTreeMap<String, Failure>,
    /// Park records of cloud panels by session, in order. A board write removes the records
    /// of panels it does not have, so they are written once no board of the session waits.
    parks: BTreeMap<String, Vec<Park>>,
    /// The session whose park records the worker writes now.
    writing_parks: Option<String>,
}

/// The park state of a cloud panel, as [`ParkedPanel`] without its borrows.
pub(super) struct Park {
    pub(super) local_id: String,
    pub(super) parked: bool,
    pub(super) status: Option<SessionStatus>,
}

enum Job {
    Board(String, Box<Queued>),
    Parks(String, Vec<Park>),
}

struct Failure {
    /// Waits for this generation, or an older one, end with `reason`.
    generation: u64,
    reason: String,
    /// The wait before this session is tried again; each failure doubles it.
    wait: Duration,
}

impl State {
    fn pending(&self) -> bool {
        self.writing.is_some()
            || self.writing_parks.is_some()
            || self.queued.values().any(|queued| queued.retry.is_none())
            || self
                .parks
                .keys()
                .any(|session_id| !self.queued.contains_key(session_id))
    }
}

#[derive(Clone)]
struct Queued {
    board: RuntimeState,
    generation: u64,
    /// When this board, whose save failed, is tried again.
    retry: Option<Instant>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Writer").finish_non_exhaustive()
    }
}

impl Writer {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `board` for `session_id` and returns its generation; a worker of `store`
    /// writes it.
    pub(super) fn queue(&self, store: &SessionStore, session_id: &str, board: RuntimeState) -> u64 {
        let mut state = self.lock();
        state.next += 1;
        let generation = state.next;
        // A newer board is tried at once, even in a session whose save failed.
        state.queued.insert(
            session_id.to_owned(),
            Queued {
                board,
                generation,
                retry: None,
            },
        );
        self.start(&mut state, store);
        drop(state);
        self.shared.changed.notify_all();
        generation
    }

    /// Whether a board is written now or was not tried yet; see [`Self::flush`].
    pub(super) fn pending(&self) -> bool {
        self.lock().pending()
    }

    fn start(&self, state: &mut State, store: &SessionStore) {
        if !state.running {
            state.running = true;
            let (writer, store) = (self.clone(), store.clone());
            std::thread::spawn(move || writer.run(&store));
        }
    }

    /// Queues park records of cloud panels of `session_id`; the worker writes them after
    /// the boards of the session that wait. A record that cannot be written is dropped.
    pub(super) fn queue_parks(&self, store: &SessionStore, session_id: &str, parks: Vec<Park>) {
        let mut state = self.lock();
        state.parks.entry(session_id.to_owned()).or_default().extend(parks);
        self.start(&mut state, store);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// The newest board queued for `session_id` that is not on disk yet.
    pub(super) fn unwritten(&self, session_id: &str) -> Option<RuntimeState> {
        let state = self.lock();
        state
            .queued
            .get(session_id)
            .or_else(|| {
                state
                    .writing
                    .as_ref()
                    .filter(|(writing, _)| writing == session_id)
                    .map(|(_, queued)| queued)
            })
            .map(|queued| queued.board.clone())
    }

    /// The generation of the newest board queued for `session_id`, written or not.
    pub(super) fn newest(&self, session_id: &str) -> Option<u64> {
        let state = self.lock();
        state
            .queued
            .get(session_id)
            .map(|queued| queued.generation)
            .or_else(|| {
                state
                    .writing
                    .as_ref()
                    .filter(|(writing, _)| writing == session_id)
                    .map(|(_, queued)| queued.generation)
            })
            .or_else(|| state.written.get(session_id).copied())
    }

    /// Waits up to `timeout` until the board of `generation`, or a newer one, of
    /// `session_id` is durable. A save of that board, or a newer one, that failed ends the
    /// wait with its reason; `None` is a wait that ran out.
    pub(super) fn wait_written(
        &self,
        session_id: &str,
        generation: u64,
        timeout: Duration,
    ) -> Result<(), Option<String>> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if state
                .written
                .get(session_id)
                .is_some_and(|written| *written >= generation)
            {
                return Ok(());
            }
            if let Some(failure) = state.failed.get(session_id)
                && failure.generation >= generation
            {
                return Err(Some(failure.reason.clone()));
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(None);
            };
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits up to `timeout` until every queued board is durable, and returns whether
    /// they are. It ends early once each board that is left has failed, so that a session
    /// that cannot be saved does not hold up the exit.
    pub(super) fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if !state.pending() {
                return state.queued.is_empty() && state.parks.is_empty();
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Drops the queued board of `session_id`, which a save that does not use the queue
    /// replaces. A wait for the dropped board ends with a reason. It is refused while the
    /// writer writes the session, so that the caller never waits for the disk.
    pub(super) fn supersede(&self, session_id: &str) -> StoreResult<()> {
        self.drop_session(
            session_id,
            "a later save that does not wait for the disk replaced it",
            false,
        )
        .map(drop)
    }

    /// As [`Self::supersede`], before the session is deleted, so that nothing writes it
    /// again; returns the board that was not written.
    pub(super) fn forget(&self, session_id: &str) -> StoreResult<Option<RuntimeState>> {
        self.drop_session(session_id, "the session was deleted", true)
    }

    fn drop_session(&self, session_id: &str, reason: &str, deleted: bool) -> StoreResult<Option<RuntimeState>> {
        let mut state = self.lock();
        if state.writing.as_ref().is_some_and(|(writing, _)| writing == session_id)
            || state.writing_parks.as_deref() == Some(session_id)
        {
            return Err(Error::State(format!(
                "Horizon is still saving session {session_id}; try again in a moment"
            )));
        }
        if deleted {
            state.parks.remove(session_id);
        }
        let Some(queued) = state.queued.remove(session_id) else {
            return Ok(None);
        };
        let failure = Failure {
            generation: state.next,
            reason: reason.to_owned(),
            wait: FIRST_RETRY,
        };
        state.failed.insert(session_id.to_owned(), failure);
        drop(state);
        self.shared.changed.notify_all();
        Ok(Some(queued.board))
    }

    fn run(&self, store: &SessionStore) {
        let _panic = PanicGuard { writer: self, store };
        loop {
            let (session_id, queued) = match self.next_job() {
                None => return,
                Some(Job::Board(session_id, queued)) => (session_id, *queued),
                Some(Job::Parks(session_id, parks)) => {
                    self.write_parks(store, &session_id, &parks);
                    continue;
                }
            };
            let saved = store.save_runtime_state_durably(&session_id, &queued.board);
            let mut state = self.lock();
            state.writing = None;
            match saved {
                Ok(()) => {
                    state.failed.remove(&session_id);
                    let written = state.written.entry(session_id).or_default();
                    *written = (*written).max(queued.generation);
                }
                Err(error) => {
                    tracing::error!(%session_id, %error, "failed to save the session; Horizon tries again");
                    let wait = state
                        .failed
                        .get(&session_id)
                        .map_or(FIRST_RETRY, |failure| (failure.wait * 2).min(LAST_RETRY));
                    let failure = Failure {
                        generation: queued.generation,
                        reason: error.to_string(),
                        wait,
                    };
                    state.failed.insert(session_id.clone(), failure);
                    // A newer board of the session replaces this one; else this one is tried
                    // again after the wait, while the boards of other sessions are written.
                    state.queued.entry(session_id).or_insert(Queued {
                        retry: Some(Instant::now() + wait),
                        ..queued
                    });
                }
            }
            drop(state);
            self.shared.changed.notify_all();
        }
    }

    fn write_parks(&self, store: &SessionStore, session_id: &str, parks: &[Park]) {
        let panels: Vec<ParkedPanel<'_>> = (parks.iter())
            .map(|park| ParkedPanel {
                local_id: &park.local_id,
                parked: park.parked,
                status: park.status.as_ref(),
            })
            .collect();
        if let Err(error) = store.record_cloud_panels(session_id, &panels) {
            tracing::warn!(%session_id, %error, "could not record the park state of a cloud");
        }
        self.lock().writing_parks = None;
        self.shared.changed.notify_all();
    }

    /// Takes the oldest board that is due, so every session is written in turn, else the
    /// park records of a session that has no board waiting; `None` ends an idle worker.
    fn next_job(&self) -> Option<Job> {
        let mut state = self.lock();
        loop {
            let now = Instant::now();
            let due = (state.queued.iter())
                .filter(|(_, queued)| queued.retry.is_none_or(|at| at <= now))
                .min_by_key(|(_, queued)| queued.generation)
                .map(|(session_id, _)| session_id.clone());
            if let Some(session_id) = due
                && let Some(queued) = state.queued.remove(&session_id)
            {
                state.writing = Some((session_id.clone(), queued.clone()));
                return Some(Job::Board(session_id, Box::new(queued)));
            }
            let parked = (state.parks.keys())
                .find(|session_id| !state.queued.contains_key(*session_id))
                .cloned();
            if let Some(session_id) = parked
                && let Some(parks) = state.parks.remove(&session_id)
            {
                state.writing_parks = Some(session_id.clone());
                return Some(Job::Parks(session_id, parks));
            }
            let wait = (state.queued.values())
                .filter_map(|queued| queued.retry.map(|at| at.saturating_duration_since(now)))
                .min()
                .unwrap_or(IDLE);
            let (next, timeout) = self
                .shared
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            if timeout.timed_out() && state.queued.is_empty() && state.parks.is_empty() {
                state.running = false;
                return None;
            }
        }
    }
}

/// If a save panics, its board counts as failed and is not tried again, and a new worker
/// writes the other boards.
struct PanicGuard<'a> {
    writer: &'a Writer,
    store: &'a SessionStore,
}

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self.writer.lock();
        if let Some((session_id, queued)) = state.writing.take() {
            let failure = Failure {
                generation: queued.generation,
                reason: "the save stopped with an internal error".into(),
                wait: FIRST_RETRY,
            };
            state.failed.insert(session_id, failure);
        }
        state.writing_parks = None;
        state.running = false;
        if !state.queued.is_empty() || !state.parks.is_empty() {
            self.writer.start(&mut state, self.store);
        }
        drop(state);
        self.writer.shared.changed.notify_all();
    }
}

/// Writes `bytes` to `path` so that a crash leaves the old file or the new one, both on
/// disk: a uniquely named temporary file is synced, renamed over `path`, and the folder is
/// synced.
pub(super) fn durable_write(path: &Path, bytes: &[u8]) -> StoreResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    sync_folder(parent)
}

/// Syncs the file at `path` and the folder that names it. Another thread can rename a new
/// file over `path` meanwhile; then it syncs again, until the name on disk names a synced file.
pub(super) fn sync_with_folder(path: &Path) -> StoreResult<()> {
    const PASSES: usize = 8;
    for _ in 0..PASSES {
        // Windows syncs only a file that is open for writing.
        let file = fs::OpenOptions::new().write(true).open(path)?;
        file.sync_all()?;
        path.parent().map_or(Ok(()), sync_folder)?;
        if same_file(&file.metadata()?, &fs::metadata(path)?) {
            return Ok(());
        }
    }
    Err(Error::State(format!(
        "{} kept changing while Horizon synced it",
        path.display()
    )))
}

#[cfg(unix)]
fn same_file(synced: &fs::Metadata, named: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    (synced.dev(), synced.ino()) == (named.dev(), named.ino())
}

/// Only a Unix host syncs a folder, and a durable wait is refused elsewhere.
#[cfg(not(unix))]
fn same_file(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    true
}

/// Syncs `folder` and the folder that names it.
pub(super) fn sync_folder_and_parent(folder: &Path) -> StoreResult<()> {
    sync_folder(folder)?;
    folder.parent().map_or(Ok(()), sync_folder)
}

fn sync_folder(folder: &Path) -> StoreResult<()> {
    #[cfg(unix)]
    fs::File::open(folder)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

#[cfg(test)]
mod tests;
