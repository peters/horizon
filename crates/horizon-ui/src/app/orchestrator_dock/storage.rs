//! The stored side of the dock: the file of the conversation of each workspace and
//! of the front desk, the record of its session in the runtime state, and the jobs
//! of the worker that read and write them.
//!
//! A conversation outlives a restart of Horizon in two parts. The record
//! `WorkspaceState.orchestrator` of the saved board holds the session id of the
//! agent program and the name of the program, and one file under
//! `<session folder>/conversations/` holds the entries. The front desk belongs to
//! no workspace, so its record stands beside the workspaces in
//! `RuntimeState.front_desk`, and its file carries the reserved key as its name.
//!
//! Neither part is written for a start that saves nothing:
//! [`OrchestratorDock::set_storage`] names the folder of the session only while
//! the app opened a saved session, as `HorizonApp::transcript_root` does for the
//! terminal transcripts.
//!
//! No frame reads or writes a file. Each load, append, compaction and delete is
//! one job of the worker of the dock, in the order that the frames asked for
//! them, and its outcome applies on a later frame
//! ([`super::worker`] and `docs/architecture/ui-thread.md`). A load fills the
//! entries of a restored conversation above the entries that this run already
//! holds, and an append writes the entries that the events settled.
//!
//! The file is untrusted when a later start reads it: `horizon_core::agent_bridge
//! ::store` bounds one line, drops a line that does not parse and counts it.

use std::path::PathBuf;

use horizon_core::agent_bridge::{Conversation, Entry, MAX_ENTRIES, ToolState, store};
use horizon_core::{Board, OrchestratorConversationState};

use super::OrchestratorDock;

/// The answer of the worker to the load of one stored conversation: the entries
/// that the file holds, and the lines that it dropped.
type Loaded = horizon_core::Result<(Vec<Entry>, u64)>;

/// The bookkeeping of the stored conversation of one session: whether the file
/// of the workspace exists, how many leading entries of the snapshot it holds,
/// and which job of the worker changes it.
///
/// A frame asks [`Storage::pending`] for one batch and sends no second job while
/// the first waits, so one conversation never has two writes on the worker. A
/// write that failed stops the writes of this run: the entries of it are lost,
/// and the dock names the failure in its log.
#[derive(Default)]
pub(super) struct Storage {
    /// The workspace keeps a conversation file, so the next start loads it.
    exists: bool,
    /// How many leading entries of the snapshot the file holds.
    count: usize,
    /// The count of the entries of the append that waits on the worker.
    queued: Option<usize>,
    /// Whether a load of the file waits on the worker.
    loading: bool,
    /// Whether a write of this file failed, so no other write is tried in this
    /// run.
    failed: bool,
}

impl Storage {
    /// The bookkeeping of a conversation whose file a restart loads.
    pub(super) fn restored() -> Self {
        Self {
            exists: true,
            loading: true,
            ..Self::default()
        }
    }

    /// Whether this workspace keeps a conversation, which its record names.
    pub(super) fn keeps(&self) -> bool {
        self.exists || self.loading
    }

    /// Whether a load of the stored conversation waits on the worker, which a
    /// test reads.
    #[cfg(test)]
    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    /// The entries that the file of `conversation` does not hold yet, with the
    /// number of entries that it holds after the write.
    ///
    /// A frame writes the entries that the events settled: a message of the
    /// person, a notice and a tool call that ended. A text of the agent waits for
    /// the end of its turn, so a partial message is never written twice. The
    /// answer is `None` while a load, an append or a failed write holds this
    /// conversation, so a frame sends at most one job for it. `flush` writes the
    /// entries that are still open, for the exit: a tool call that runs then
    /// reads as interrupted on the next start.
    pub(super) fn pending(&self, conversation: &Conversation, flush: bool) -> Option<(Vec<Entry>, usize)> {
        if self.loading || self.failed || (self.queued.is_some() && !flush) {
            return None;
        }
        let snapshot = conversation.snapshot();
        let entries = &snapshot.entries;
        let stored = self.queued.unwrap_or(self.count).min(entries.len());
        let end = if flush {
            entries.len()
        } else {
            settled_end(entries, snapshot.busy, stored)
        };
        (end > stored).then(|| (entries[stored..end].to_vec(), end))
    }

    /// Records the append of the entries that reach `count`, which the worker
    /// writes now.
    pub(super) fn queued(&mut self, count: usize) {
        self.queued = Some(count);
    }

    /// Applies the answer of the worker to the append that waited.
    pub(super) fn finished(&mut self, written: bool) {
        match self.queued.take() {
            Some(count) if written => {
                self.count = count;
                self.exists = true;
            }
            Some(_) => self.failed = true,
            None => {}
        }
    }

    /// Applies the entries that the worker read from the file: they stand above
    /// the entries that this run already holds.
    pub(super) fn loaded(&mut self, conversation: &mut Conversation, entries: Vec<Entry>) {
        self.loading = false;
        self.exists = true;
        let count = entries.len();
        conversation.prepend(entries);
        self.count = count.min(conversation.snapshot().entries.len());
    }
}

/// The entries from `from` that the file of a conversation holds: every entry
/// that the events settled. A text of the agent that a turn still writes is
/// open, and so is a tool call that did not end.
fn settled_end(entries: &[Entry], busy: bool, from: usize) -> usize {
    let mut end = from;
    while let Some(entry) = entries.get(end) {
        if !settled(entry, busy) {
            break;
        }
        end += 1;
    }
    end
}

/// Whether one entry is complete enough to write: a message of the person, a
/// notice, a tool call that ended, and a text of the agent whose turn ended.
fn settled(entry: &Entry, busy: bool) -> bool {
    match entry {
        Entry::Person { .. }
        | Entry::Notice { .. }
        | Entry::Card { .. }
        | Entry::Proposal { .. }
        | Entry::WorkItem { .. }
        | Entry::WorkerStarted { .. }
        | Entry::WorkerMessage { .. }
        | Entry::WorkerReport { .. }
        | Entry::WorkerAsk { .. } => true,
        Entry::Tool { state, .. } => *state != ToolState::Running,
        Entry::Agent { .. } => !busy,
    }
}

impl OrchestratorDock {
    /// The folder that keeps the conversation of each workspace of the session
    /// that this app opened, or `None` while nothing is saved. A session switch
    /// or a start without a saved session forgets the conversations of the board
    /// that goes; their files stay in the folder of their own session.
    ///
    /// Every bridge of those conversations goes to the worker for its stop, as it
    /// does on every other path that loses a workspace: a drop of a bridge only
    /// closes its input, and the program would outlive the board that it speaks
    /// for.
    pub(in crate::app) fn set_storage(&mut self, root: Option<PathBuf>) {
        if self.storage == root {
            return;
        }
        self.storage = root;
        self.stop_and_clear();
    }

    /// The path of the conversation of `local_id`, or `None` while the dock
    /// stores nothing.
    pub(in crate::app) fn conversation_path(&self, local_id: &str) -> Option<PathBuf> {
        self.storage
            .as_deref()
            .map(|root| store::conversation_path(root, local_id))
    }

    /// The name of the file of the program that this dock starts, which the
    /// record of a conversation names.
    pub(in crate::app) fn program_name(&self) -> &str {
        self.program
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
    }

    /// Queues the load of the stored conversation of each workspace that keeps
    /// one and has no conversation of this run, and the same load of the front
    /// desk. The entries arrive on a later frame; until then the dock shows the
    /// empty or the live state that it has.
    pub(in crate::app) fn restore_conversations(&mut self, board: &Board) {
        if self.storage.is_none() {
            return;
        }
        let program = self.program_name().to_owned();
        let mut wanted: Vec<(String, OrchestratorConversationState)> = board
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let record = workspace.orchestrator.clone()?;
                Some((workspace.local_id.clone(), record))
            })
            .collect();
        // The front desk belongs to no workspace, so its record comes from the
        // runtime state of this session and not from the board.
        if let Some(record) = self.front_desk().record().cloned() {
            wanted.push((super::FRONT_DESK.to_owned(), record));
        }
        for (local_id, record) in wanted {
            let Some(path) = self.conversation_path(&local_id) else {
                continue;
            };
            if self.sessions.contains_key(&local_id) {
                continue;
            }
            let session = super::session::Session::restored(record.session_id, &record.program, &program);
            // The chip of the workers is the choice of the person, so the record of
            // the conversation brings it back with the session.
            self.set_workers_allowed(&local_id, record.allowed_workers);
            self.sessions.insert(local_id.clone(), session);
            let loaded_id = local_id.clone();
            let handed = self.worker.then(
                move || {
                    let loaded = store::load(&path, MAX_ENTRIES);
                    if let Err(error) = store::compact(&path) {
                        tracing::warn!(%error, "failed to compact a stored conversation");
                    }
                    loaded
                },
                move |dock, outcome| dock.apply_loaded(&loaded_id, outcome),
            );
            if !handed {
                // No worker took the load, so the stored entries stay in their
                // file and this conversation writes nothing this run, as a load
                // that failed does.
                self.apply_loaded(&local_id, None);
            }
        }
    }

    /// Applies the answer of the worker to the load of one conversation. A
    /// workspace that closed while the load ran keeps nothing.
    fn apply_loaded(&mut self, local_id: &str, outcome: Option<Loaded>) {
        let Some(session) = self.sessions.get_mut(local_id) else {
            return;
        };
        match outcome {
            Some(Ok((entries, dropped))) => {
                if dropped > 0 {
                    tracing::warn!(%local_id, dropped, "the line of a stored conversation was dropped");
                }
                session.loaded(entries);
            }
            Some(Err(error)) => tracing::warn!(%local_id, %error, "failed to read a stored conversation"),
            None => tracing::warn!(%local_id, "the load of a stored conversation panicked"),
        }
    }

    /// Keeps the record of each conversation of this board in step with the
    /// session that its program named, and the record of the front desk in the
    /// state of this dock, which the caller saves beside the workspaces. The
    /// answer says whether a record changed, so the caller marks the runtime state
    /// for its next save. Nothing changes while the dock stores nothing: an
    /// unsaved start writes no record.
    pub(in crate::app) fn sync_conversation_records(&mut self, board: &mut Board, program: &str) -> bool {
        if self.storage.is_none() || program.is_empty() {
            return false;
        }
        let mut wanted: Vec<(String, Option<OrchestratorConversationState>)> = Vec::new();
        for (local_id, session) in &self.sessions {
            let allowed = self.workers_allowed(local_id).to_vec();
            wanted.push((local_id.clone(), session.record(program, &allowed)));
        }
        let mut changed = false;
        for (local_id, record) in wanted {
            if local_id == super::FRONT_DESK {
                if self.front_desk().record() != record.as_ref() {
                    self.front_desk_mut().set_record(record);
                    changed = true;
                }
                continue;
            }
            let Some(workspace_id) = board.workspace_id_by_local_id(&local_id) else {
                continue;
            };
            let Some(workspace) = board.workspace_mut(workspace_id) else {
                continue;
            };
            if workspace.orchestrator != record {
                workspace.orchestrator = record;
                changed = true;
            }
        }
        changed
    }

    /// Queues one append for each conversation that holds entries the file does
    /// not have. A frame sends at most one job for a conversation, and its
    /// outcome applies on a later frame.
    pub(in crate::app) fn queue_conversation_appends(&mut self) {
        self.queue_appends(false);
    }

    /// Queues the entries that the exit leaves open, so a tool call that still
    /// runs reads as interrupted on the next start. The caller flushes the worker
    /// within the bound of the exit.
    pub(in crate::app) fn flush_conversation_appends(&mut self) {
        self.queue_appends(true);
    }

    fn queue_appends(&mut self, flush: bool) {
        if self.storage.is_none() {
            return;
        }
        let mut ready: Vec<(String, PathBuf, Vec<Entry>, usize)> = Vec::new();
        for (local_id, session) in &self.sessions {
            let Some((entries, count)) = session.pending_append(flush) else {
                continue;
            };
            let Some(path) = self.conversation_path(local_id) else {
                continue;
            };
            ready.push((local_id.clone(), path, entries, count));
        }
        for (local_id, path, entries, count) in ready {
            if let Some(session) = self.sessions.get_mut(&local_id) {
                session.append_queued(count);
            }
            let writer_id = local_id.clone();
            let handed = self.worker.then(
                move || store::append(&path, &entries).map_err(|error| error.to_string()),
                move |dock, outcome| dock.apply_appended(&writer_id, outcome),
            );
            if !handed {
                // No worker took the write, so its entries reached no file: this
                // conversation stops writing for this run, as a failed write does.
                self.apply_appended(&local_id, None);
            }
        }
    }

    /// Applies the answer of the worker to the append of one conversation.
    fn apply_appended(&mut self, local_id: &str, outcome: Option<Result<(), String>>) {
        let written = match outcome {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                tracing::warn!(%local_id, %error, "failed to write a stored conversation");
                false
            }
            None => {
                tracing::warn!(%local_id, "a write of a stored conversation panicked");
                false
            }
        };
        if let Some(session) = self.sessions.get_mut(local_id) {
            session.append_finished(written);
        }
    }

    /// Removes the file of the conversation of a workspace that closes, on the
    /// worker: the conversation of a workspace that goes is gone for good.
    pub(in crate::app) fn delete_conversation(&mut self, local_id: &str) {
        let Some(path) = self.conversation_path(local_id) else {
            return;
        };
        // A delete that no worker took leaves the file where it is, which is the
        // state of a delete that failed.
        let _ = self.worker.write(move || {
            if let Err(error) = store::remove(&path) {
                tracing::warn!(%error, "failed to remove a stored conversation");
            }
        });
    }

    /// The path of the stored conversation of `local_id`, which a test reads.
    #[cfg(test)]
    pub(in crate::app) fn stored_path(&self, local_id: &str) -> Option<PathBuf> {
        self.conversation_path(local_id)
    }
}

/// The folder of a conversation is a part of the folder that a delete of a saved
/// session removes: `SessionStore::delete_session` removes
/// `sessions/<session id>/`, and the conversations stand below it.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// The folder of the conversations of a session is a part of the folder that
    /// the delete of that session removes.
    #[test]
    fn the_folder_of_a_conversation_goes_with_the_delete_of_its_session() {
        let home = horizon_core::HorizonHome::from_root(PathBuf::from("/tmp/horizon-home"));
        assert_eq!(
            home.session_conversations_dir("session-1").parent(),
            Some(home.session_dir("session-1").as_path()),
            "a session folder that goes takes its conversations with it"
        );
    }
}
