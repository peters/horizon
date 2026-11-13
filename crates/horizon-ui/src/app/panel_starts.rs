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
    #[cfg(test)]
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
            // The panel that stands is the identity that the board answers with:
            // its local id names the actor of the program that this answer
            // starts, so a request of that program finds this panel again.
            let Some(local_id) = self.board.panel(pending.id).map(|panel| panel.local_id.clone()) else {
                // The panel left the board before its request went out, so no
                // answer can start it and nothing waits for one.
                self.panel_starts.waiting.remove(&pending.id);
                continue;
            };
            let request = StartRequest {
                id: pending.id,
                local_id,
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
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use horizon_core::{
        AgentSessionBinding, PanelId, PanelKind, PanelOptions, PanelState, RuntimeState, WorkspaceState,
    };

    use crate::app::browser_requests::actor_panel;
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

    /// The kinds of agent panel that Set up can start.
    const SETUP_KINDS: [PanelKind; 5] = [
        PanelKind::Codex,
        PanelKind::Claude,
        PanelKind::Pi,
        PanelKind::Grok,
        PanelKind::OpenCode,
    ];

    /// The options of an agent panel whose program writes the actor of its own
    /// environment to `marker`, one line for each start of the program. The
    /// program is a script, so a test reads exactly what an agent program reads,
    /// also when the launch of its kind adds arguments of its own.
    fn agent_options_that_say_their_actor(kind: PanelKind, marker: &Path) -> PanelOptions {
        let program = marker.with_extension("sh");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$HORIZON_BROWSER_ACTOR\" >> '{}'\n",
                marker.display()
            ),
        )
        .expect("the program of the test");
        let mut permissions = std::fs::metadata(&program)
            .expect("the program of the test")
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&program, permissions).expect("an executable program");
        PanelOptions {
            kind,
            command: Some(program.display().to_string()),
            ..PanelOptions::default()
        }
    }

    fn actors_written(marker: &Path) -> Vec<String> {
        std::fs::read_to_string(marker)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The actors that the program of a panel wrote, once it wrote `count` of
    /// them. A program starts on a worker thread, so a test waits for its output.
    fn wait_for_actors(marker: &Path, count: usize) -> Vec<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            let actors = actors_written(marker);
            if actors.len() >= count {
                return actors;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!(
            "the program of the panel did not write {count} actors to {}: {:?}",
            marker.display(),
            actors_written(marker)
        );
    }

    fn local_id_of(app: &crate::app::HorizonApp, id: PanelId) -> String {
        app.board.panel(id).expect("the panel of the test").local_id.clone()
    }

    /// Every agent program that Set up can start keeps one actor from the first
    /// line of its program: the actor of its environment names the panel that
    /// stands, so a card, a browser tool and a device tool find that panel.
    #[test]
    fn every_agent_kind_of_setup_starts_with_the_actor_of_its_panel() {
        let (temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        for kind in SETUP_KINDS {
            let marker = temp.path().join(format!("actor-{kind:?}.txt"));
            let created = app
                .create_panel_with_options(agent_options_that_say_their_actor(kind, &marker), workspace)
                .expect("an agent panel");
            let local_id = local_id_of(&app, created);
            app.settle_panel_starts();
            let actor = wait_for_actors(&marker, 1).remove(0);
            assert_eq!(
                local_id_of(&app, created),
                local_id,
                "the started {kind:?} panel keeps the identity that it waited with"
            );
            assert_eq!(
                actor,
                horizon_core::browser_actor(&local_id),
                "the program of a {kind:?} panel carries the actor of its own panel"
            );
            assert_eq!(
                actor_panel(&app.board, &actor).map(|found| found.panel_id),
                Some(created),
                "the board finds the {kind:?} panel that asks"
            );
        }
    }

    /// Two panels of one kind in one folder are two callers. A value that both of
    /// them carried would name no panel at all, because the board holds one actor
    /// for each local id.
    #[test]
    fn two_agent_panels_of_one_kind_in_one_folder_hold_two_actors() {
        let (temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let (first_marker, second_marker) = (temp.path().join("first.txt"), temp.path().join("second.txt"));
        let first = app
            .create_panel_with_options(
                agent_options_that_say_their_actor(PanelKind::Codex, &first_marker),
                workspace,
            )
            .expect("the first agent panel");
        let second = app
            .create_panel_with_options(
                agent_options_that_say_their_actor(PanelKind::Codex, &second_marker),
                workspace,
            )
            .expect("the second agent panel");
        let (first_local_id, second_local_id) = (local_id_of(&app, first), local_id_of(&app, second));
        app.settle_panel_starts();
        let first_actor = wait_for_actors(&first_marker, 1).remove(0);
        let second_actor = wait_for_actors(&second_marker, 1).remove(0);
        assert_ne!(first_actor, second_actor, "two panels of one kind are two callers");
        assert_eq!(first_actor, horizon_core::browser_actor(&first_local_id));
        assert_eq!(second_actor, horizon_core::browser_actor(&second_local_id));
        assert_eq!(
            actor_panel(&app.board, &first_actor).map(|found| found.panel_id),
            Some(first)
        );
        assert_eq!(
            actor_panel(&app.board, &second_actor).map(|found| found.panel_id),
            Some(second)
        );
    }

    /// A panel keeps its actor when its session binds and when the person starts
    /// its program again: the identity belongs to the panel, not to a start.
    #[test]
    fn a_panel_that_binds_its_session_and_starts_again_keeps_its_actor() {
        let (temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let marker = temp.path().join("actor-restart.txt");
        let created = app
            .create_panel_with_options(agent_options_that_say_their_actor(PanelKind::Codex, &marker), workspace)
            .expect("an agent panel");
        let local_id = local_id_of(&app, created);
        app.settle_panel_starts();
        let first = wait_for_actors(&marker, 1).remove(0);
        let binding = AgentSessionBinding::new(PanelKind::Codex, "thread-7".to_owned(), None, None, None);
        app.board
            .panel_mut(created)
            .expect("the panel")
            .set_session_binding(Some(binding));

        app.run_panel_notice_action(created, crate::terminal_widget::PanelNoticeAction::StartProgramAgain);
        {
            let _frame = horizon_core::ui_thread::FrameScope::enter();
            app.apply_panel_transitions();
        }
        app.settle_panel_restarts();
        let second = wait_for_actors(&marker, 2).remove(1);

        assert_eq!(first, second, "the program of both starts carries one actor");
        assert_eq!(second, horizon_core::browser_actor(&local_id));
        assert_eq!(
            actor_panel(&app.board, &second).map(|found| found.panel_id),
            Some(created),
            "the board finds the panel after the restart too"
        );
    }
}
