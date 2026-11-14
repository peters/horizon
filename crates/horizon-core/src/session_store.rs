mod deletion_notice;
mod index_file;
mod lease;
mod model;
mod runtime_index;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
pub(crate) mod writer;

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak, mpsc::Receiver};
use std::time::Duration;

use uuid::Uuid;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::retired_panel_kinds::RetiredPanels;
use crate::runtime_state::{RUNTIME_STATE_VERSION, RuntimeState};
use index_file::{IndexChange, IndexFile};
use model::{ProfileSnapshot, SessionIndex, SessionMeta, StoredSession};

pub use deletion_notice::SessionDeletionNotice;

pub use model::{
    ResolvedSession, SessionLease, SessionOpenDisposition, SessionSummary, StartupChooser, StartupDecision,
    StartupPromptReason,
};

const SESSION_INDEX_VERSION: u32 = 1;
const SESSION_META_VERSION: u32 = 1;
const SESSION_LEASE_VERSION: u32 = 1;
const LEASE_STALE_AFTER_MILLIS: i64 = 15_000;

/// What the removal of a session does with a board that this Horizon cannot read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnreadableBoard {
    /// Stop the removal and keep every file of the session. A later Horizon still reads
    /// the board, and an older Horizon must not destroy it.
    Keep,
    /// Remove the session. Only a person who saw that reason asks for this.
    Remove,
}

#[derive(Clone, Debug)]
pub struct SessionStore {
    home: HorizonHome,
    config_path: PathBuf,
    profile_id: String,
    index: runtime_index::IndexCache,
    writer: writer::Writer,
    /// The stores that this window holds. The last one that goes ends the worker of
    /// this store, so a store that a window lets go keeps no file open and no thread
    /// idle.
    handles: Arc<writer::Handles>,
    /// The sessions whose board came from an older build or lost panels of a
    /// removed kind, so the first write of this build keeps the old file.
    boards_of_older_builds: Arc<Mutex<BTreeSet<String>>>,
}

impl SessionStore {
    #[must_use]
    pub fn new(home: HorizonHome, config_path: PathBuf) -> Self {
        let profile_id = profile_id_for_home(&home, &config_path);
        let writer = writer::Writer::default();
        Self {
            home,
            config_path,
            profile_id,
            index: runtime_index::IndexCache::default(),
            handles: Arc::new(writer::Handles::new(writer.clone())),
            writer,
            boards_of_older_builds: Arc::default(),
        }
    }

    /// The store that the worker of this store holds: the same queue, index and home,
    /// and a marker of its own, so it keeps no store of the window alive.
    fn worker_copy(&self) -> Self {
        Self {
            home: self.home.clone(),
            config_path: self.config_path.clone(),
            profile_id: self.profile_id.clone(),
            index: self.index.clone(),
            writer: self.writer.clone(),
            handles: Arc::new(writer::Handles::detached()),
            boards_of_older_builds: Arc::clone(&self.boards_of_older_builds),
        }
    }

    /// The marker that the worker watches, so it ends when the window let the store
    /// go.
    fn handles_marker(&self) -> Weak<writer::Handles> {
        Arc::downgrade(&self.handles)
    }

    #[must_use]
    pub fn home(&self) -> &HorizonHome {
        &self.home
    }

    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Decide how Horizon should open the current profile on startup.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored profile snapshot cannot be read, or if
    /// the selected session cannot be created or loaded from disk.
    pub fn prepare_startup(&self, config: &Config) -> Result<StartupDecision> {
        crate::ui_thread::blocking("the sessions of this profile are read");
        let profile = self.load_profile_snapshot()?;

        if profile.sessions.is_empty() {
            let session = self.create_new_session(config)?;
            return Ok(StartupDecision::Open {
                disposition: SessionOpenDisposition::New,
                session: Box::new(session),
            });
        }

        let selected = profile
            .last_session_id
            .as_ref()
            .and_then(|session_id| {
                profile
                    .sessions
                    .iter()
                    .find(|session| session.summary.session_id == *session_id)
            })
            .or_else(|| profile.sessions.first());

        if let Some(session) = selected {
            if session.is_live {
                return Ok(self.startup_chooser(StartupPromptReason::LiveConflict, profile.sessions));
            }

            let disposition = if session.has_stale_lease {
                SessionOpenDisposition::Recover
            } else {
                SessionOpenDisposition::Resume
            };
            let resolved = self.load_existing_session(&session.summary.session_id)?;
            return Ok(StartupDecision::Open {
                disposition,
                session: Box::new(resolved),
            });
        }

        if profile.sessions.len() > 1 {
            return Ok(self.startup_chooser(StartupPromptReason::MultipleRecoverable, profile.sessions));
        }

        let session = self.load_existing_session(&profile.sessions[0].summary.session_id)?;
        Ok(StartupDecision::Open {
            disposition: SessionOpenDisposition::Resume,
            session: Box::new(session),
        })
    }

    /// Create and persist a fresh session for the current profile.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial runtime state or session metadata
    /// cannot be serialized or written to disk.
    pub fn create_new_session(&self, config: &Config) -> Result<ResolvedSession> {
        crate::ui_thread::blocking("a session folder is made and written");
        let runtime_state = RuntimeState::from_config(config);
        self.create_session_from_runtime(runtime_state)
    }

    /// Clone an existing session's runtime state and transcripts into a new session.
    ///
    /// # Errors
    ///
    /// Returns an error if the source session cannot be loaded or if the new
    /// session data cannot be persisted.
    pub fn duplicate_session(&self, source_session_id: &str) -> Result<ResolvedSession> {
        crate::ui_thread::blocking("the folder of a session is copied");
        let (mut runtime_state, left_out) = self
            .load_runtime_reporting(source_session_id)?
            .ok_or_else(|| Error::State(format!("missing runtime state for session {source_session_id}")))?;
        runtime_state.regenerate_browser_local_ids();
        let mut session = self.create_session_from_runtime(runtime_state)?;
        session.retired_panels = left_out;
        copy_directory_recursive(
            &self.home.session_transcripts_dir(source_session_id),
            &self.home.session_transcripts_dir(&session.session_id),
        )?;
        Ok(session)
    }

    /// Load an existing session for resumption.
    ///
    /// # Errors
    ///
    /// Returns an error if the session runtime state or metadata cannot be read.
    pub fn resume_session(&self, session_id: &str) -> Result<ResolvedSession> {
        crate::ui_thread::blocking("a session of this profile is read");
        self.load_existing_session(session_id)
    }

    /// Load an existing session after explicitly taking over a stale lease.
    ///
    /// # Errors
    ///
    /// Returns an error if the session runtime state or metadata cannot be read.
    pub fn take_over_session(&self, session_id: &str) -> Result<ResolvedSession> {
        crate::ui_thread::blocking("a session of this profile is read");
        self.load_existing_session(session_id)
    }

    /// List saved sessions for the current profile using the same ordering as startup.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored session index or metadata cannot be read.
    pub fn list_profile_sessions(&self) -> Result<Vec<SessionSummary>> {
        crate::ui_thread::blocking("the sessions of this profile are listed");
        Ok(self
            .load_profile_snapshot()?
            .sessions
            .into_iter()
            .map(|session| session.summary)
            .collect())
    }

    /// Delete an inactive saved session from disk, and queue its removal from the profile
    /// index for the session writer.
    ///
    /// A board that this Horizon cannot read keeps the whole session, so an older Horizon
    /// cannot destroy the board of a later one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnreadableBoard`] if its board is not one this Horizon can read,
    /// which [`Self::delete_session_with_unreadable_board`] answers. Returns another error
    /// if the session does not belong to the current profile, is still live, or its files
    /// cannot be removed.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        crate::ui_thread::blocking("a session folder and its browser profiles are removed");
        self.remove_session(session_id, UnreadableBoard::Keep)
    }

    /// Delete an inactive saved session whose board this Horizon cannot read.
    ///
    /// Only a person who saw that reason asks for this: the board goes with the session,
    /// and a later Horizon cannot read it again. The browser profile folders of that board
    /// stay, because only the board names them.
    ///
    /// # Errors
    ///
    /// Returns an error if the session does not belong to the current profile,
    /// is still live, or its files cannot be removed.
    pub fn delete_session_with_unreadable_board(&self, session_id: &str) -> Result<()> {
        self.remove_session(session_id, UnreadableBoard::Remove)
    }

    fn remove_session(&self, session_id: &str, unreadable: UnreadableBoard) -> Result<()> {
        let session = self
            .load_profile_snapshot()?
            .sessions
            .into_iter()
            .find(|session| session.summary.session_id == session_id)
            .ok_or_else(|| Error::State(format!("session {session_id} was not found for this profile")))?;
        if session.is_live {
            return Err(Error::State(format!(
                "cannot delete live session {session_id} while it is still active"
            )));
        }
        // The writer leaves the session alone and keeps its board until the delete succeeds.
        let direct = self.writer.reserve(session_id)?;
        self.remove_browser_profiles(session_id, unreadable)?;
        self.index.forget(session_id);
        remove_dir_if_exists(&self.home.session_dir(session_id))?;
        self.writer
            .queue_index(self, IndexChange::Remove(session_id.to_owned()));
        direct.forget();
        Ok(())
    }

    /// Remove the browser profile folders that the board of `session_id` names.
    ///
    /// Only the board names those folders, so a board that this Horizon cannot read keeps
    /// them. `unreadable` says whether such a board stops the removal of the session.
    ///
    /// # Errors
    ///
    /// Returns an error if the board cannot be read and `unreadable` is
    /// [`UnreadableBoard::Keep`], or if a profile folder cannot be removed.
    fn remove_browser_profiles(&self, session_id: &str, unreadable: UnreadableBoard) -> Result<()> {
        let runtime_state = match (self.load_runtime(session_id), unreadable) {
            (Ok(Some(runtime_state)), _) => runtime_state,
            (Ok(None), _) => return Ok(()),
            (Err(error), UnreadableBoard::Keep) => {
                return Err(Error::UnreadableBoard {
                    session_id: session_id.to_owned(),
                    reason: error.to_string(),
                });
            }
            (Err(error), UnreadableBoard::Remove) => {
                tracing::warn!("session {session_id} is removed without its browser profiles: {error}");
                return Ok(());
            }
        };
        for workspace in &runtime_state.workspaces {
            for panel in &workspace.panels {
                if panel.kind != crate::panel::PanelKind::Browser {
                    continue;
                }
                let saved_profile = panel
                    .browser_profile
                    .as_ref()
                    .and_then(|profile| profile.session_id.as_deref());
                // A shared profile holds the sign-ins of a person and of the
                // agents of this Horizon, so the delete of one saved session
                // leaves it. Only the throwaway folder of a private panel goes.
                if saved_profile.is_some() {
                    continue;
                }
                let browser_config = panel.browser_config_for_restore(&runtime_state.browser);
                // A private panel keeps the folder of the private pages, whose
                // name this page decides by the profile that it saves.
                let mut browser_config = browser_config;
                browser_config.profile = Some(horizon_browser::PRIVATE_PROFILE.to_string());
                let profile_dir = crate::browser::profile_dir_for_home(&browser_config, &self.home, &panel.local_id);
                remove_dir_if_exists(&profile_dir)?;
            }
        }
        Ok(())
    }

    /// The folder name of every browser panel that a saved session of this
    /// Horizon home names.
    ///
    /// The sweep of the throwaway folders of private panels reads this set, so a
    /// folder that a saved session still names for a panel that comes back is
    /// never removed.
    #[must_use]
    pub fn saved_browser_profile_folders(&self) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        let Ok(entries) = std::fs::read_dir(self.home.sessions_dir()) else {
            return names;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().to_string();
            let Ok(Some(runtime_state)) = self.load_runtime(&session_id) else {
                continue;
            };
            for workspace in &runtime_state.workspaces {
                for panel in &workspace.panels {
                    if panel.kind == crate::panel::PanelKind::Browser {
                        names.insert(horizon_browser_control::paths::safe_local_id(&panel.local_id));
                    }
                }
            }
        }
        names
    }

    /// Persist the runtime state and refreshed metadata for a session, and queue its touch of
    /// the profile index for the session writer. It replaces a board of the session that is
    /// queued and not written yet.
    ///
    /// # Errors
    ///
    /// The board or metadata cannot be serialized or written, or the session writer
    /// writes this session now.
    pub fn save_runtime_state(&self, session_id: &str, runtime_state: &RuntimeState) -> Result<()> {
        crate::ui_thread::blocking("a session board is written and synced");
        let _direct = self.writer.supersede(session_id)?;
        self.write_runtime_state(session_id, runtime_state, atomic_write)?;
        self.writer.queue_index(self, IndexChange::Touch(session_id.to_owned()));
        Ok(())
    }

    /// Writes the lease of a session on the session writer thread, in the order
    /// of the other writes of this Horizon. A window that takes or refreshes a
    /// lease never waits for the disk, and the answer of the write reaches a later
    /// frame through the returned receiver.
    #[must_use]
    pub fn queue_lease(&self, lease: SessionLease) -> Receiver<std::result::Result<(), String>> {
        self.writer
            .queue_lease(self, writer::LeaseWrite::Write(Box::new(lease)))
    }

    /// Removes the lease of `session_id` on the session writer thread, with the
    /// same order and the same answer.
    #[must_use]
    pub fn queue_lease_release(&self, session_id: &str) -> Receiver<std::result::Result<(), String>> {
        self.writer
            .queue_lease(self, writer::LeaseWrite::Remove(session_id.to_owned()))
    }

    /// The lease of `session_id` of this Horizon, in memory, for a write that runs
    /// on the session writer.
    #[must_use]
    pub fn new_lease(&self, session_id: &str) -> SessionLease {
        SessionLease::new(session_id.to_owned())
    }

    /// Writes `lease` as it stands. Only the session writer calls it.
    pub(super) fn write_lease(&self, lease: &SessionLease) -> Result<()> {
        let json = serde_json::to_vec_pretty(lease).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_lease_path(&lease.session_id), &json)
    }

    /// Queues the board of `session_id` for a durable save on a worker thread and returns
    /// its generation; the caller never waits for the disk. A read of the session returns
    /// this board until it is written, and a later queue replaces it if it is not written.
    #[must_use = "the generation tells when the board is durable"]
    pub fn queue_runtime_state(&self, session_id: &str, runtime_state: RuntimeState) -> u64 {
        self.writer.queue(self, session_id, runtime_state)
    }

    /// The generation of the newest board queued for `session_id`, written or not.
    #[must_use]
    pub fn queued_runtime_state(&self, session_id: &str) -> Option<u64> {
        self.writer.newest(session_id)
    }

    /// Waits up to `timeout` until the board of `generation` of `session_id`, or a newer
    /// one, is durable: its content and its name are on disk.
    ///
    /// # Errors
    /// A save of that board or a newer one failed, with its reason; or the wait ran out
    /// (`None`). The writer keeps trying a failed save. Only a Unix host can sync a
    /// folder, so elsewhere this refuses at once.
    pub fn wait_runtime_state_durable(
        &self,
        session_id: &str,
        generation: u64,
        timeout: Duration,
    ) -> std::result::Result<(), Option<String>> {
        if let Err(error) = require_directory_durability() {
            return Err(Some(error.to_string()));
        }
        self.writer.wait_written(session_id, generation, timeout)
    }

    /// Whether a board is written now or not tried yet; a failed one does not hold an exit.
    #[must_use]
    pub fn runtime_states_pending(&self) -> bool {
        self.writer.pending()
    }

    /// Reads the recovery notices of this profile on the session writer thread.
    ///
    /// The answer arrives through the receiver, so a frame never waits for the
    /// folders of the notices.
    #[must_use]
    pub fn saved_session_deletion_notice_queued(
        &self,
    ) -> std::sync::mpsc::Receiver<std::result::Result<Option<String>, String>> {
        self.writer.queue_notice(self, writer::NoticeWrite::Read)
    }

    /// Saves a recovery notice on the session writer thread, which syncs every
    /// folder of the notice.
    #[must_use]
    pub fn save_session_deletion_notice_queued(
        &self,
        notice: &SessionDeletionNotice,
    ) -> std::sync::mpsc::Receiver<std::result::Result<Option<String>, String>> {
        self.writer
            .queue_notice(self, writer::NoticeWrite::Save(Box::new(notice.clone())))
    }

    /// Removes the notice that a person reviewed, on the session writer thread.
    #[must_use]
    pub fn acknowledge_session_deletion_notice_queued(
        &self,
        expected: &str,
    ) -> std::sync::mpsc::Receiver<std::result::Result<Option<String>, String>> {
        self.writer
            .queue_notice(self, writer::NoticeWrite::Acknowledge(expected.to_owned()))
    }

    /// Waits up to `timeout` until every queued board is durable, as before an exit.
    #[must_use]
    pub fn flush_runtime_states(&self, timeout: Duration) -> bool {
        self.writer.flush(timeout)
    }

    /// As [`Self::save_runtime_state`], each file synced before and after its rename, and the
    /// index changes queued `ahead` of it written with its touch. Only the session writer calls it.
    fn save_runtime_state_durably(
        &self,
        session_id: &str,
        runtime_state: &RuntimeState,
        ahead: &[IndexChange],
    ) -> Result<()> {
        self.write_runtime_state(session_id, runtime_state, writer::durable_write)?;
        // A new session folder is named in the sessions folder, a new one of those in the root.
        writer::sync_folder_and_parent(&self.home.sessions_dir())?;
        let index = self.write_index_changes(ahead.iter().chain([&IndexChange::Touch(session_id.to_owned())]))?;
        if index.names(&self.profile_id, session_id) {
            return Ok(());
        }
        Err(Error::State(format!(
            "the index on disk does not name session {session_id}"
        )))
    }

    /// Writes the board, the runtime index and the metadata of `session_id` with `write`.
    ///
    /// A board that an older build wrote, or one that lost the panels of a
    /// removed kind, keeps a copy beside its file at this first write, so a person
    /// who goes back to the older build finds the board that it saved.
    fn write_runtime_state(
        &self,
        session_id: &str,
        runtime_state: &RuntimeState,
        write: fn(&Path, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let runtime_path = self.home.session_runtime_path(session_id);
        let meta_path = self.home.session_meta_path(session_id);
        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.keep_the_board_before_this_build(session_id, &runtime_path);
        let runtime_yaml = runtime_state.to_yaml()?;
        write(&runtime_path, runtime_yaml.as_bytes())?;
        self.index_runtime_state(session_id, runtime_state, &runtime_yaml);
        let existing_meta = self.load_session_meta(session_id).unwrap_or_else(|_| {
            SessionMeta::new(
                session_id.to_string(),
                self.profile_id.clone(),
                self.config_path.display().to_string(),
                runtime_state,
                current_unix_millis(),
            )
        });
        let meta = existing_meta.updated(runtime_state, current_unix_millis());
        let meta_yaml = serde_yaml::to_string(&meta).map_err(|error| Error::State(error.to_string()))?;
        write(&meta_path, meta_yaml.as_bytes())
    }

    /// Copies the board of `session_id` to `runtime.yaml.before-<version>` once,
    /// when it came from an older build or lost the panels of a removed kind. The
    /// copy is made in the write that the caller already does, and never again: the
    /// file next to the board is for the person who goes back to the older build.
    fn keep_the_board_before_this_build(&self, session_id: &str, runtime_path: &Path) {
        let waits_for_a_copy = {
            let mut waiting = self
                .boards_of_older_builds
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            waiting.remove(session_id)
        };
        if !waits_for_a_copy || !runtime_path.is_file() {
            return;
        }
        let before = self.home.session_runtime_before_path(session_id, RUNTIME_STATE_VERSION);
        if before.exists() {
            return;
        }
        // The copy is what an older build would read again, and the save of this
        // build must not fail because of it: a board that cannot be copied is
        // written, and the log says which file stayed behind.
        match fs::copy(runtime_path, &before) {
            Ok(_) => tracing::info!(
                session = session_id,
                kept = %before.display(),
                "the board of an older build stays beside the board of this build"
            ),
            Err(error) => tracing::warn!(
                session = session_id,
                kept = %before.display(),
                %error,
                "the board of an older build cannot be kept"
            ),
        }
    }

    /// Remembers that the next write of `session_id` keeps the board that it read.
    fn note_board_of_an_older_build(&self, session_id: &str) {
        self.boards_of_older_builds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session_id.to_owned());
    }

    /// Writes `changes` in order to the session index under its locks, and returns the index
    /// on disk after them. Only the session writer calls it, never the UI thread.
    fn write_index_changes<'a>(&self, changes: impl IntoIterator<Item = &'a IndexChange>) -> Result<SessionIndex> {
        IndexFile::at(&self.home.session_index_path()).change(|index| {
            for change in changes {
                change.apply(index, &self.profile_id);
            }
        })
    }

    /// Persist a new session from an already prepared runtime state snapshot, and queue its
    /// touch of the profile index for the session writer.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime state or session metadata cannot be
    /// serialized or written to disk.
    pub fn create_session_from_runtime(&self, mut runtime_state: RuntimeState) -> Result<ResolvedSession> {
        crate::ui_thread::blocking("a session folder is made and written");
        runtime_state.ensure_local_ids();
        let session_id = Uuid::new_v4().to_string();
        let now = current_unix_millis();
        let runtime_path = self.home.session_runtime_path(&session_id);
        let transcript_root = self.home.session_transcripts_dir(&session_id);
        let meta = SessionMeta::new(
            session_id.clone(),
            self.profile_id.clone(),
            self.config_path.display().to_string(),
            &runtime_state,
            now,
        );

        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&transcript_root)?;

        let runtime_yaml = runtime_state.to_yaml()?;
        atomic_write(&runtime_path, runtime_yaml.as_bytes())?;
        self.index_runtime_state(&session_id, &runtime_state, &runtime_yaml);
        let meta_yaml = serde_yaml::to_string(&meta).map_err(|error| Error::State(error.to_string()))?;
        atomic_write(&self.home.session_meta_path(&session_id), meta_yaml.as_bytes())?;

        self.writer.queue_index(self, IndexChange::Touch(session_id.clone()));

        Ok(ResolvedSession {
            session_id,
            runtime_state,
            runtime_state_path: runtime_path,
            transcript_root,
            meta,
            retired_panels: RetiredPanels::default(),
        })
    }

    fn load_existing_session(&self, session_id: &str) -> Result<ResolvedSession> {
        let (runtime_state, retired_panels) = self
            .load_runtime_reporting(session_id)?
            .ok_or_else(|| Error::State(format!("missing runtime state for session {session_id}")))?;
        let meta = self.load_session_meta(session_id)?;

        Ok(ResolvedSession {
            session_id: session_id.to_string(),
            runtime_state,
            runtime_state_path: self.home.session_runtime_path(session_id),
            transcript_root: self.home.session_transcripts_dir(session_id),
            meta,
            retired_panels,
        })
    }

    fn startup_chooser(&self, reason: StartupPromptReason, sessions: Vec<StoredSession>) -> StartupDecision {
        StartupDecision::Choose(StartupChooser {
            reason,
            config_path: self.config_path.display().to_string(),
            sessions: sessions.into_iter().map(|session| session.summary).collect(),
        })
    }

    fn load_profile_snapshot(&self) -> Result<ProfileSnapshot> {
        crate::ui_thread::blocking("the sessions, boards and leases of this profile are read");
        // Read before the file: no change is missed, but one written meanwhile can show an older order once.
        let unwritten = self.writer.unwritten_index_changes();
        let mut index = IndexFile::at(&self.home.session_index_path()).load()?;
        for change in &unwritten {
            change.apply(&mut index, &self.profile_id);
        }
        let last_session_id = index
            .profile(&self.profile_id)
            .and_then(|profile| profile.last_session_id.clone());
        let sessions_dir = self.home.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(ProfileSnapshot {
                last_session_id,
                sessions: Vec::new(),
            });
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = self.load_session_meta(&session_id) else {
                continue;
            };
            if meta.profile_id != self.profile_id {
                continue;
            }

            let lease = self.load_session_lease(&session_id)?;
            let is_live = lease.as_ref().is_some_and(SessionLease::is_live);
            let has_stale_lease = lease.is_some() && !is_live;

            sessions.push(StoredSession {
                summary: SessionSummary::from_meta(&meta, is_live),
                is_live,
                has_stale_lease,
            });
        }

        sessions.sort_by(|left, right| {
            if Some(left.summary.session_id.as_str()) == last_session_id.as_deref() {
                return std::cmp::Ordering::Less;
            }
            if Some(right.summary.session_id.as_str()) == last_session_id.as_deref() {
                return std::cmp::Ordering::Greater;
            }
            right.summary.last_active_at.cmp(&left.summary.last_active_at)
        });

        Ok(ProfileSnapshot {
            last_session_id,
            sessions,
        })
    }

    fn load_session_meta(&self, session_id: &str) -> Result<SessionMeta> {
        crate::ui_thread::blocking("the metadata of a session is read");
        let contents = fs::read_to_string(self.home.session_meta_path(session_id))?;
        let mut meta =
            serde_yaml::from_str::<SessionMeta>(&contents).map_err(|error| Error::State(error.to_string()))?;
        meta.version = SESSION_META_VERSION;
        Ok(meta)
    }
}

fn derive_session_label(runtime_state: &RuntimeState) -> Option<String> {
    runtime_state
        .workspaces
        .iter()
        .find(|workspace| !workspace.name.is_empty())
        .map(|workspace| workspace.name.clone())
}

/// The profile id of a run: the hash of the settings path that the run of this
/// home has always used.
///
/// A run of the usual home keeps the id that release v0.2.7 and the `main` build
/// gave it, which is the id of `<home>/config.yaml`. The settings of this build
/// are `<home>/config.json`, and the hash of that name would be a second profile
/// with an empty board: the sessions, the last session and the browser profiles of
/// the person would look gone, and a person who goes back to an older build would
/// not find what this build wrote. A run that names its settings file keeps the
/// id of that path, as before.
fn profile_id_for_home(home: &HorizonHome, config_path: &Path) -> String {
    let stable_path = if config_path == home.config_path() {
        let root = fs::canonicalize(home.root()).unwrap_or_else(|_| home.root().to_path_buf());
        root.join("config.yaml")
    } else {
        fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf())
    };
    stable_state_key(&stable_path.to_string_lossy())
}

fn stable_state_key(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn current_unix_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            // The system call answers on every Unix. macOS keeps no
            // `/etc/hostname`, so a file read gave every Mac `localhost`.
            let name = rustix::system::uname().nodename().to_string_lossy().trim().to_string();
            (!name.is_empty()).then_some(name)
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

pub(crate) fn require_directory_durability() -> std::io::Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Durable directory updates are unsupported on this platform",
        ))
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, bytes)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }

    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
