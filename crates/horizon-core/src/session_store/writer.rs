//! Durable session saves on one worker thread, so the UI thread never waits for a disk.
//!
//! The UI queues the newest board of a session; a later queue of the same session replaces
//! a board that is not written yet. The worker writes each board, its metadata and the
//! session index so that a crash cannot leave a session without its content: each file is
//! written to a temporary file, synced, renamed into place, and its folder is synced. A
//! read of a session that is queued or being written returns the queued board, so a reader
//! never sees an older one. A save that fails is kept and tried again later, while the
//! saves of other sessions go on.
use super::SessionStore;
use crate::RuntimeState;
use crate::error::Result as StoreResult;
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
    /// The board the worker writes now was replaced or its session deleted, so its result
    /// counts for nothing.
    dropped: bool,
    /// The newest generation written for each session.
    written: BTreeMap<String, u64>,
    next: u64,
    running: bool,
    /// The last failed save of each session, until a save of that session succeeds.
    failed: BTreeMap<String, Failure>,
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
        self.writing.is_some() || self.queued.values().any(|queued| queued.retry.is_none())
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
                return state.queued.is_empty();
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
    /// replaces, and waits for a write of the session that runs now. A wait for a dropped
    /// board ends with a reason.
    pub(super) fn supersede(&self, session_id: &str) {
        self.drop_session(session_id, "a later save that does not wait for the disk replaced it");
    }

    /// As [`Self::supersede`], as before the session is deleted, so that nothing writes it
    /// again.
    pub(super) fn forget(&self, session_id: &str) {
        self.drop_session(session_id, "the session was deleted");
    }

    fn drop_session(&self, session_id: &str, reason: &str) {
        let mut state = self.lock();
        let writing = state.writing.as_ref().is_some_and(|(writing, _)| writing == session_id);
        if state.queued.remove(session_id).is_some() || writing {
            state.dropped |= writing;
            let generation = state.next;
            state.failed.insert(
                session_id.to_owned(),
                Failure {
                    generation,
                    reason: reason.to_owned(),
                    wait: FIRST_RETRY,
                },
            );
            self.shared.changed.notify_all();
        }
        while state.writing.as_ref().is_some_and(|(writing, _)| writing == session_id) {
            state = self.shared.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn run(&self, store: &SessionStore) {
        let _panic = PanicGuard { writer: self, store };
        loop {
            let Some((session_id, queued)) = self.next_board() else {
                return;
            };
            let saved = store.save_runtime_state_durably(&session_id, &queued.board);
            let mut state = self.lock();
            // A save that does not use the queue, or a delete, may have replaced this one.
            state.writing = None;
            let current = !std::mem::take(&mut state.dropped);
            match (saved, current) {
                (_, false) => {}
                (Ok(()), true) => {
                    state.failed.remove(&session_id);
                    let written = state.written.entry(session_id).or_default();
                    *written = (*written).max(queued.generation);
                }
                (Err(error), true) => {
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

    /// Takes the oldest board that is due, so every session is written in turn; `None`
    /// ends an idle worker.
    fn next_board(&self) -> Option<(String, Queued)> {
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
                return Some((session_id, queued));
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
            if timeout.timed_out() && state.queued.is_empty() {
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
        state.dropped = false;
        state.running = false;
        if !state.queued.is_empty() {
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

/// Syncs the file at `path` and the folder that names it.
pub(super) fn sync_with_folder(path: &Path) -> StoreResult<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
    path.parent().map_or(Ok(()), sync_folder)
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
mod tests {
    use super::*;
    use crate::HorizonHome;

    fn store() -> (tempfile::TempDir, SessionStore) {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(
            HorizonHome::from_root(temp.path().join("home")),
            temp.path().join("config.yaml"),
        );
        (temp, store)
    }

    fn board(active: &str) -> RuntimeState {
        RuntimeState {
            active_workspace_local_id: Some(active.into()),
            ..RuntimeState::default()
        }
    }

    fn on_disk(store: &SessionStore) -> Option<String> {
        let text = std::fs::read_to_string(store.home().session_runtime_path("session")).ok()?;
        serde_yaml::from_str::<RuntimeState>(&text)
            .unwrap()
            .active_workspace_local_id
    }

    #[test]
    #[cfg(unix)]
    fn a_queued_board_is_read_back_before_it_is_written_and_then_is_durable_on_disk() {
        let (_temp, store) = store();
        store.save_runtime_state("session", &board("first")).unwrap();
        let generation = store.queue_runtime_state("session", board("second"));
        // Whatever the worker did yet, a reader never gets the older board.
        let read = store.load_runtime("session").unwrap().unwrap();
        assert_eq!(read.active_workspace_local_id.as_deref(), Some("second"));
        assert_eq!(
            store.wait_runtime_state_durable("session", generation, Duration::from_secs(30)),
            Ok(())
        );
        assert_eq!(on_disk(&store).as_deref(), Some("second"));
        assert!(store.writer.unwritten("session").is_none(), "nothing is left queued");
    }

    #[test]
    #[cfg(unix)]
    fn a_newer_board_replaces_one_that_is_not_written_yet() {
        let (_temp, store) = store();
        let boards: Vec<u64> = (0..20)
            .map(|step| store.queue_runtime_state("session", board(&format!("step-{step}"))))
            .collect();
        let newest = *boards.last().unwrap();
        assert_eq!(store.queued_runtime_state("session"), Some(newest));
        assert!(store.flush_runtime_states(Duration::from_secs(30)));
        assert_eq!(
            on_disk(&store).as_deref(),
            Some("step-19"),
            "the newest board is on disk"
        );
        // An older generation counts as written once a newer board of the session is.
        assert_eq!(
            store.wait_runtime_state_durable("session", boards[0], Duration::from_secs(1)),
            Ok(())
        );
    }

    #[test]
    fn a_session_switch_reads_the_board_the_writer_still_holds() {
        let (_temp, store) = store();
        store.save_runtime_state("session", &board("old")).unwrap();
        let _ = store.queue_runtime_state("session", board("new"));
        let resumed = store.resume_session("session").unwrap();
        assert_eq!(resumed.runtime_state.active_workspace_local_id.as_deref(), Some("new"));
        assert!(store.flush_runtime_states(Duration::from_secs(30)));
    }

    #[test]
    #[cfg(unix)]
    fn a_session_that_cannot_be_saved_does_not_hold_up_the_others() {
        let (_temp, store) = store();
        store.save_runtime_state("broken", &board("old")).unwrap();
        // A folder where the board goes refuses every save of this session.
        let runtime = store.home().session_runtime_path("broken");
        std::fs::remove_file(&runtime).unwrap();
        std::fs::create_dir(&runtime).unwrap();
        let broken = store.queue_runtime_state("broken", board("never"));
        assert!(matches!(
            store.wait_runtime_state_durable("broken", broken, Duration::from_secs(30)),
            Err(Some(_))
        ));
        let healthy = store.queue_runtime_state("session", board("saved"));
        assert_eq!(
            store.wait_runtime_state_durable("session", healthy, Duration::from_secs(30)),
            Ok(())
        );
        assert_eq!(on_disk(&store).as_deref(), Some("saved"));
        assert!(
            !store.flush_runtime_states(Duration::from_secs(30)),
            "the exit does not wait for a session that keeps failing"
        );
        assert!(!store.runtime_states_pending(), "a failed board is not pending");
        store.writer.forget("broken");
        assert!(store.flush_runtime_states(Duration::ZERO));
    }

    #[test]
    fn a_save_outside_the_queue_replaces_the_queued_board() {
        let (_temp, store) = store();
        let _ = store.queue_runtime_state("session", board("queued"));
        store.save_runtime_state("session", &board("direct")).unwrap();
        assert!(store.flush_runtime_states(Duration::from_secs(30)));
        assert_eq!(on_disk(&store).as_deref(), Some("direct"));
        let read = store.load_runtime("session").unwrap().unwrap();
        assert_eq!(read.active_workspace_local_id.as_deref(), Some("direct"));
    }

    #[test]
    #[cfg(not(unix))]
    fn a_durable_save_is_refused_where_folders_cannot_be_synced() {
        let (_temp, store) = store();
        let generation = store.queue_runtime_state("session", board("queued"));
        assert!(matches!(
            store.wait_runtime_state_durable("session", generation, Duration::from_secs(30)),
            Err(Some(_))
        ));
        assert!(store.flush_runtime_states(Duration::from_secs(30)));
        assert_eq!(on_disk(&store).as_deref(), Some("queued"), "the save itself still runs");
    }
}
