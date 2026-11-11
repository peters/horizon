//! The panels whose process waits for the preparation of the coordination
//! worker.
//!
//! A panel's process needs reads of its transcript, of its working directory and
//! of the saved work of its agent, and the probe of `script`. None of that runs
//! on a frame: the frame creates the panel, which says that Horizon prepares it,
//! and asks the worker for the answer. The answer starts the process, so a panel
//! of a restored board appears at once and fills in when it is ready.

use std::collections::HashMap;

use horizon_core::{PanelId, PanelOptions, PanelStart, StartRequest, prepare_panel_start};

use super::HorizonApp;

/// The panels of this board that wait for the preparation of their process,
/// with the options that each one was created from.
#[derive(Default)]
pub(in crate::app) struct PanelStarts {
    waiting: HashMap<PanelId, PanelOptions>,
}

impl PanelStarts {
    /// The board that these panels belong to was replaced: a later answer names
    /// a panel of a board that no longer stands.
    pub(in crate::app) fn clear(&mut self) {
        self.waiting.clear();
    }

    /// How many panels wait for their preparation.
    pub(in crate::app) fn waiting_count(&self) -> usize {
        self.waiting.len()
    }
}

impl HorizonApp {
    /// Asks the coordination worker for the preparation of each panel that the
    /// frame created, once for each panel. A request that the worker takes
    /// answers on a later frame.
    pub(in crate::app) fn queue_panel_starts(&mut self) {
        for pending in self.board.take_pending_starts() {
            if self
                .panel_starts
                .waiting
                .insert(pending.id, pending.options.clone())
                .is_some()
            {
                continue;
            }
            let request = StartRequest {
                id: pending.id,
                local_id: pending.options.local_id.clone().unwrap_or_default(),
                opts: pending.options,
            };
            let id = pending.id;
            self.browser_create_host.io.then(
                move || prepare_panel_start(request),
                move |app, start| {
                    app.apply_panel_start(id, start);
                },
            );
        }
    }

    /// Queues the preparation of every panel that waits and applies its answer,
    /// for a test that judges a panel right after the frame that created it.
    #[cfg(test)]
    pub(in crate::app) fn settle_panel_starts(&mut self) {
        self.queue_panel_starts();
        self.settle_browser_host_io();
    }

    /// Starts the process of a panel with the answer of the worker. An answer
    /// that names a panel that no longer waits, because the person closed it or
    /// another session replaced the board, is dropped.
    fn apply_panel_start(&mut self, id: PanelId, start: Option<PanelStart>) {
        let Some(options) = self.panel_starts.waiting.remove(&id) else {
            return;
        };
        let Some(start) = start else {
            tracing::error!(
                panel_id = id.0,
                "the coordination worker could not prepare the start of a panel"
            );
            return;
        };
        if let Err(error) = self.board.start_panel(id, &options, start) {
            tracing::error!(panel_id = id.0, %error, "could not start a panel that waited for its preparation");
        }
        self.mark_runtime_dirty();
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelState, RuntimeState, WorkspaceState};

    use crate::app::test_support::{run_app_frame, test_app};

    fn terminal_panel(local_id: &str) -> PanelState {
        PanelState {
            local_id: local_id.to_owned(),
            name: local_id.to_owned(),
            kind: PanelKind::Shell,
            rows: 6,
            cols: 40,
            ..PanelState::default()
        }
    }

    /// The saved board of two terminals, whose first panel has a saved screen.
    fn saved_board() -> (RuntimeState, String) {
        let first = terminal_panel("first-panel");
        let local_id = first.local_id.clone();
        (
            RuntimeState {
                workspaces: vec![WorkspaceState {
                    local_id: "workspace".to_owned(),
                    name: "workspace".to_owned(),
                    panels: vec![first, terminal_panel("second-panel")],
                    ..WorkspaceState::default()
                }],
                ..RuntimeState::default()
            },
            local_id,
        )
    }

    /// The visible screen of the panel of `local_id`, as text.
    fn screen_of(app: &crate::app::HorizonApp, local_id: &str) -> String {
        let panel = app.board.panel_id_by_local_id(local_id).expect("the panel");
        String::from_utf8_lossy(
            &app.board
                .panel(panel)
                .and_then(horizon_core::Panel::terminal)
                .expect("the terminal of the panel")
                .screen_replay_bytes(),
        )
        .into_owned()
    }

    #[test]
    fn a_restored_board_shows_every_panel_and_its_replay_text() {
        let (temp, mut app) = test_app();
        let transcript_root = temp.path().join("transcripts");
        std::fs::create_dir_all(&transcript_root).expect("transcript root");
        let (state, local_id) = saved_board();
        std::fs::write(
            transcript_root.join(format!("{local_id}.bin")),
            b"restored marker text\r\n",
        )
        .expect("saved screen");
        app.transcript_root = Some(transcript_root);
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.restore_startup_runtime_state(&state);
            assert_eq!(app.board.panels.len(), 2, "every panel of the board stands at once");
            let waiting = screen_of(&app, &local_id);
            assert!(
                waiting.contains("Preparing this panel"),
                "the panel says that it waits for its saved screen: {waiting}"
            );
            assert!(
                !waiting.contains("restored marker text"),
                "no panel read its transcript on the frame that restored it: {waiting}"
            );
        }
        let ctx = egui::Context::default();
        run_app_frame(&ctx, &mut app);
        app.settle_panel_starts();
        assert_eq!(
            app.panel_starts.waiting_count(),
            0,
            "the answers of the worker started every panel"
        );
        let screen = screen_of(&app, &local_id);
        assert!(
            screen.contains("restored marker text"),
            "the restored panel draws its saved screen: {screen}"
        );
        for panel in &app.board.panels {
            assert!(panel.terminal().is_some(), "every restored panel runs");
        }
    }
}
