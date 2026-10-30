//! Deletion of the transcript files of a closed panel. The files are removed
//! on the coordination worker, in the order the panels closed, so a slow disk
//! never stalls a close that an agent asked for. A panel that comes back with
//! the same local id before its delete runs keeps its transcript: a restore
//! marks the panels it carries before any wait can run the queued deletes, and
//! the job removes the files only while the id is still there.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use horizon_core::PanelTranscript;

use super::HorizonApp;

/// The local ids whose transcript delete is still wanted.
#[derive(Clone, Default)]
pub(super) struct PendingTranscriptDeletes {
    wanted: Arc<Mutex<BTreeSet<String>>>,
}

impl PendingTranscriptDeletes {
    /// Records that the transcript of `local_id` is to be deleted, and returns
    /// the set the coordinated job checks before it removes the files.
    fn want(&self, local_id: &str) -> Self {
        self.wanted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(local_id.to_string());
        self.clone()
    }

    /// Removes the files only while the delete is still wanted: a panel that
    /// came back with the same local id first keeps its transcript.
    fn delete(&self, local_id: &str, transcript: &PanelTranscript) {
        let wanted = self
            .wanted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(local_id);
        if !wanted {
            return;
        }
        if let Err(error) = transcript.delete_all() {
            tracing::warn!(%local_id, "failed to delete panel transcript: {error}");
        }
    }

    /// A board that comes back carries the panels it saved again: the deletes
    /// of their local ids remove nothing.
    fn keep(&self, carried: &BTreeSet<String>) {
        let mut wanted = self.wanted.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        wanted.retain(|local_id| !carried.contains(local_id));
    }
}

impl HorizonApp {
    /// Deletes the transcript of a panel that just closed, on the coordination
    /// worker. The panel left the board and its teardown is requested before
    /// this call, so the delete runs after both, and a later panel that reuses
    /// the local id keeps its files.
    pub(super) fn delete_closed_transcript(&mut self, local_id: &str, transcript: PanelTranscript) {
        let wanted = self.transcript_deletes.want(local_id);
        let local_id = local_id.to_string();
        self.browser_create_host.io.write(move || {
            wanted.delete(&local_id, &transcript);
        });
    }

    /// A board that comes back carries the panels it saved again: a delete
    /// queued for one of them removes nothing.
    pub(super) fn keep_restored_transcripts(&self) {
        let carried = self.board.panels.iter().map(|panel| panel.local_id.clone()).collect();
        self.transcript_deletes.keep(&carried);
    }

    /// The same for a saved board that Horizon has not restored yet. A restore
    /// marks the panels its saved board carries before any wait of an exit or
    /// of a replacement runs the queued deletes, so a slow disk cannot remove
    /// the transcript of a panel that comes back.
    pub(super) fn keep_transcripts_of_saved_board(&self, runtime_state: &horizon_core::RuntimeState) {
        let carried = runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panels.iter())
            .map(|panel| panel.local_id.clone())
            .collect();
        self.transcript_deletes.keep(&carried);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::{PanelKind, RuntimeState, WorkspaceState};

    /// Writes the transcript files of a shell panel that carries `local_id`
    /// and returns them.
    fn transcript_files(root: &std::path::Path, local_id: &str) -> (PanelTranscript, std::path::PathBuf) {
        std::fs::create_dir_all(root).expect("transcript root");
        let transcript = PanelTranscript::for_panel(PanelKind::Shell, Some(root.to_path_buf()), local_id)
            .expect("a shell panel keeps a transcript");
        std::fs::write(transcript.history_path(), b"old output\r\n").expect("history");
        std::fs::write(transcript.session_path(), b"Script started on a fixture\n").expect("session");
        let history = transcript.history_path();
        (transcript, history)
    }

    /// The board a restore, a relaunch or a session switch returns: it carries
    /// the panel `local_id` again.
    fn restored_board(local_id: &str) -> RuntimeState {
        RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "restored".into(),
                panels: vec![horizon_core::PanelState {
                    local_id: local_id.to_string(),
                    kind: PanelKind::Shell,
                    ..horizon_core::PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        }
    }

    #[test]
    fn a_transcript_delete_never_removes_the_transcript_of_a_relaunched_panel() {
        let (temp, mut app) = test_app();
        let root = temp.path().join("transcripts");
        let (transcript, history) = transcript_files(&root, "relaunched");
        // Hold the coordination worker, as a disk that does not answer does.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        app.delete_closed_transcript("relaunched", transcript.clone());
        assert!(app.browser_create_host.io.busy(), "the delete waits for the worker");
        assert!(history.exists(), "the closed panel keeps its files until the job runs");

        // The panel comes back with the same local id before the job runs.
        app.activate_ephemeral_session(&restored_board("relaunched"));
        release.send(()).expect("the worker waits");
        app.settle_browser_host_io();

        assert!(
            history.exists() && transcript.session_path().exists(),
            "a panel that came back with the same local id keeps its transcript"
        );
    }

    #[test]
    fn a_transcript_delete_removes_the_files_of_a_panel_that_stays_closed() {
        let (temp, mut app) = test_app();
        let root = temp.path().join("transcripts");
        let (transcript, history) = transcript_files(&root, "closed");
        app.delete_closed_transcript("closed", transcript.clone());
        assert!(history.exists(), "the delete waits for the worker");
        app.settle_browser_host_io();
        assert!(!history.exists(), "the transcript of a closed panel is removed");
        assert!(!transcript.session_path().exists());
    }

    /// Holds the coordination worker as a slow disk does, and releases it from
    /// another thread while a restore waits for it. The restore runs on this
    /// thread, so the release must not wait for it.
    fn hold_the_worker_until(app: &mut HorizonApp) -> std::thread::JoinHandle<()> {
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = release.send(());
        })
    }

    /// A restore waits for the coordination worker before it replaces the board.
    /// A delete that is still queued then must not remove the transcript of a
    /// panel that the restored board carries again.
    #[cfg(feature = "cloud-workspaces")]
    #[test]
    fn a_cloud_restore_marks_the_panels_it_carries_before_the_wait_runs_the_queued_delete() {
        let (temp, mut app) = test_app();
        let root = temp.path().join("transcripts");
        let (transcript, history) = transcript_files(&root, "restored");
        let releasing = hold_the_worker_until(&mut app);
        app.delete_closed_transcript("restored", transcript.clone());

        app.restore_cloud_prototype_board(&restored_board("restored"))
            .expect("restored board");
        releasing.join().expect("the release thread ends");

        app.settle_browser_host_io();
        assert!(
            history.exists() && transcript.session_path().exists(),
            "a panel that a restored board carries again keeps its transcript"
        );
    }

    /// The same for a session switch, which waits for the worker before it
    /// restores the next session.
    #[test]
    fn a_session_switch_marks_the_panels_it_restores_before_the_wait_runs_the_queued_delete() {
        let (temp, mut app) = test_app();
        let root = temp.path().join("transcripts");
        let (transcript, history) = transcript_files(&root, "restored");
        let target = app
            .session_store
            .create_session_from_runtime(restored_board("restored"))
            .expect("target session");
        let releasing = hold_the_worker_until(&mut app);
        app.delete_closed_transcript("restored", transcript.clone());

        app.activate_runtime_session(&egui::Context::default(), &target);
        releasing.join().expect("the release thread ends");

        assert!(app.pending_session_switch.is_none(), "the switch completed");
        app.settle_browser_host_io();
        assert!(
            history.exists() && transcript.session_path().exists(),
            "a panel that the next session carries keeps its transcript"
        );
    }
}
