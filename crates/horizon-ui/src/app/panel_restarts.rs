//! The restarts of panels, whose pre-flight runs on the coordination worker.
//!
//! A restart reads the working directory of its panel, the saved conversation of
//! its agent and the key of its machine, and it waits for the old process to
//! end. The frame copies what the reads need, the worker answers with the launch,
//! and the frame starts the process. A restore of a machine terminal and the
//! "Reconnect" of the person take the same path, so both keep their attempt
//! count and their delay.

use std::collections::HashMap;

use horizon_core::{PanelId, PanelStart, RestartPreflight, finish_restart, prepare_restart};

use super::HorizonApp;

/// Why a panel restarts, so that a failure reaches the caller that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum RestartCause {
    /// The person picked "Reconnect", a resume of a conversation, or a restart.
    Panel,
    /// A machine terminal reaches its tmux session again after a lost link.
    Reconnect,
}

/// The panels whose restart runs, and why each one restarts.
#[derive(Default)]
pub(in crate::app) struct PanelRestarts {
    in_flight: HashMap<PanelId, RestartCause>,
}

impl PanelRestarts {
    /// The board that these restarts belong to was replaced: a later answer
    /// names a panel of a board that no longer stands.
    pub(in crate::app) fn clear(&mut self) {
        self.in_flight.clear();
    }

    /// How many restarts run, for a test that judges the board.
    #[cfg(test)]
    pub(in crate::app) fn running(&self) -> usize {
        self.in_flight.len()
    }
}

impl HorizonApp {
    /// Asks the coordination worker for the pre-flight of the restart of
    /// `panel_id`. The same panel never restarts twice at once.
    pub(in crate::app) fn request_panel_restart(&mut self, panel_id: PanelId, cause: RestartCause) {
        if self.panel_restarts.in_flight.contains_key(&panel_id) {
            return;
        }
        let Some(panel) = self.board.panel_mut(panel_id) else {
            return;
        };
        let request = match panel.restart_request(
            self.session_store.home().clone(),
            self.template_config.features.keep_agents_through_restart,
        ) {
            Ok(Some(request)) => request,
            // A page relaunches itself, as it reads no file, and a panel that
            // runs no process has nothing to start again.
            Ok(None) => {
                self.forget_panel_render_caches(panel_id);
                return;
            }
            Err(error) => {
                self.restart_failed(panel_id, cause, &error);
                return;
            }
        };
        self.panel_restarts.in_flight.insert(panel_id, cause);
        let board = self.browser_create_host.io.board_fence();
        self.browser_create_host.io.then(
            move || board.holds().then(|| prepare_restart(request)),
            move |app, answer| app.apply_restart_preflight(panel_id, answer.flatten()),
        );
    }

    /// The answer of the pre-flight: a panel that needs no teardown restarts
    /// here, and one whose old process must end first is torn down on this
    /// frame and its launch resolves on the worker.
    fn apply_restart_preflight(&mut self, panel_id: PanelId, answer: Option<horizon_core::Result<RestartPreflight>>) {
        let Some(cause) = self.panel_restarts.in_flight.get(&panel_id).copied() else {
            return;
        };
        let Some(answer) = answer else {
            self.restart_failed(panel_id, cause, &"the preparation of the restart did not finish");
            return;
        };
        let mut preflight = match answer {
            Ok(preflight) => preflight,
            Err(error) => {
                self.restart_failed(panel_id, cause, &error);
                return;
            }
        };
        let work_brief_wanted = preflight.work_brief_wanted();
        if let Some(ready) = preflight.take_ready() {
            self.apply_restart_start(panel_id, Some(Ok(ready)));
            return;
        }
        let Some(pending) = preflight.into_pending() else {
            self.panel_restarts.in_flight.remove(&panel_id);
            return;
        };
        if let Err(error) = self.board.begin_restart(panel_id, work_brief_wanted) {
            self.restart_failed(panel_id, cause, &error);
            return;
        }
        self.browser_create_host.io.then(
            move || finish_restart(pending),
            move |app, start| app.apply_restart_start(panel_id, start),
        );
    }

    /// Starts the process of the restart with the answer of the worker, or says
    /// why it could not.
    fn apply_restart_start(&mut self, panel_id: PanelId, start: Option<horizon_core::Result<PanelStart>>) {
        let Some(cause) = self.panel_restarts.in_flight.remove(&panel_id) else {
            return;
        };
        let started = match start {
            Some(Ok(start)) => self.board.apply_restart(panel_id, start),
            Some(Err(error)) => Err(error),
            None => Err(horizon_core::Error::State(
                "the launch of the restart did not resolve".into(),
            )),
        };
        match started {
            Ok(()) => {
                // The new process draws a new screen, so the grid of the lost
                // one must not come back from the render cache.
                self.forget_panel_render_caches(panel_id);
                if cause == RestartCause::Reconnect {
                    tracing::info!(panel_id = panel_id.0, "a machine terminal reconnects after a lost link");
                }
            }
            Err(error) => self.restart_failed(panel_id, cause, &error),
        }
    }

    /// Says why the restart of `panel_id` did not happen. A machine terminal
    /// that cannot start again is not a lost link, so it stays disconnected
    /// until the person acts.
    fn restart_failed(&mut self, panel_id: PanelId, cause: RestartCause, error: &dyn std::fmt::Display) {
        self.panel_restarts.in_flight.remove(&panel_id);
        match cause {
            RestartCause::Panel => {
                tracing::error!(panel_id = panel_id.0, %error, "failed to restart panel");
            }
            RestartCause::Reconnect => {
                tracing::warn!(panel_id = panel_id.0, %error, "a machine terminal did not reconnect");
                self.reconnects.cancel(panel_id);
                if let Some(panel) = self.board.panel_mut(panel_id) {
                    panel.show_disconnected();
                }
            }
        }
    }

    fn forget_panel_render_caches(&mut self, panel_id: PanelId) {
        self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
        self.panel_render_caches.editor_preview_cache.remove(&panel_id);
        self.panel_render_caches.browser_ui_state.remove(&panel_id);
        self.panel_render_caches.device_ui_state.remove(&panel_id);
    }

    /// Applies the outcomes of the restarts that the worker answered, for a test
    /// that judges a restart right after the frame that asked for it.
    #[cfg(test)]
    pub(in crate::app) fn settle_panel_restarts(&mut self) {
        self.settle_browser_host_io();
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelId, PanelKind, PanelResume, SshConnection, SshConnectionStatus};

    use super::RestartCause;
    use crate::app::test_support::test_app;

    /// Waits until the program of `panel_id` ended and the app knows it.
    fn wait_for_the_end(app: &mut crate::app::HorizonApp, panel_id: PanelId) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Some(panel) = app.board.panel_mut(panel_id) {
                panel.process_output();
                if panel.program_end().is_some() {
                    return;
                }
            } else {
                panic!("the panel of the test left the board");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the program of the panel did not end");
    }

    /// C10: the two controls of an ended program do what they say. "Start again"
    /// asks for one restart of the same panel, and "Close" closes it.
    #[test]
    fn an_ended_program_starts_again_or_closes_only_when_the_person_asks() {
        use crate::terminal_widget::PanelNoticeAction;

        let (_temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let options = horizon_core::PanelOptions {
            kind: PanelKind::Command,
            command: Some("/bin/sh".to_owned()),
            args: vec!["-c".to_owned(), "exit 4".to_owned()],
            ..horizon_core::PanelOptions::default()
        };
        let created = app
            .create_panel_with_options(options, workspace)
            .expect("a command panel");
        app.settle_panel_starts();
        wait_for_the_end(&mut app, created);
        assert_eq!(
            app.board.panel(created).and_then(horizon_core::Panel::program_end),
            Some(horizon_core::ProgramEnd { exit_status: Some(4) }),
            "the panel reports the end of its program"
        );
        assert!(
            app.panels_to_close.is_empty() && app.panels_to_restart.is_empty(),
            "an ended program starts nothing again and closes nothing by itself"
        );

        app.run_panel_notice_action(created, PanelNoticeAction::StartProgramAgain);
        assert_eq!(
            app.panels_to_restart,
            vec![created],
            "the control asks for one restart of the panel"
        );
        let frame = horizon_core::ui_thread::FrameScope::enter();
        app.apply_panel_transitions();
        drop(frame);
        app.settle_panel_restarts();
        assert_eq!(
            app.board.panel(created).and_then(horizon_core::Panel::program_end),
            None,
            "the new program of the panel holds the panel"
        );

        app.run_panel_notice_action(created, PanelNoticeAction::ClosePanel);
        assert_eq!(app.panels_to_close, vec![created], "the control closes the panel");
        app.run_panel_notice_action(created, PanelNoticeAction::None);
        assert_eq!(app.panels_to_close, vec![created], "no other action closes a panel");
    }

    #[test]
    fn a_restart_of_the_person_runs_on_the_coordination_worker_and_keeps_its_attempts() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let options = horizon_core::PanelOptions {
            kind: PanelKind::Ssh,
            command: Some("/bin/sh".to_owned()),
            args: vec!["-c".to_owned(), "exit 0".to_owned()],
            ssh_connection: Some(SshConnection {
                host: "203.0.113.10".to_owned(),
                ..SshConnection::default()
            }),
            ..horizon_core::PanelOptions::default()
        };
        let created = app
            .create_panel_with_options(options, workspace)
            .expect("a terminal of the machine");
        app.settle_panel_starts();
        let frame = horizon_core::ui_thread::FrameScope::enter();
        app.request_panel_restart(created, RestartCause::Reconnect);
        assert_eq!(app.panel_restarts.running(), 1, "the restart of the panel runs");
        drop(frame);
        app.settle_panel_restarts();
        assert_eq!(app.panel_restarts.running(), 0, "the restart of the panel ended");
        assert_eq!(
            app.board.panel(created).and_then(horizon_core::Panel::ssh_status),
            Some(SshConnectionStatus::Connecting),
            "the restart started the process of the panel again"
        );
        let _ = (PanelId(0), PanelResume::Fresh);
    }
}
