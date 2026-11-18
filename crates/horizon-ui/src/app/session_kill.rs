//! The ask before a close ends a tmux session, and the worker that ends it.
//!
//! A terminal of a workspace that is placed on a machine names a tmux session on
//! that machine. Closing the panel by the person ends that session, so the work
//! of a closed panel does not fill the machine; quitting Horizon ends none, so
//! the sessions outlive the window.
//!
//! The close of one panel and the close of the panels of a whole workspace ask
//! first, in the same words. What ends no tmux session of a machine closes at
//! once.
//!
//! The close asks for the kill and the next frame hands it to the coordination
//! worker, which runs `ssh` and waits at most ten seconds. A close never waits
//! for a machine, and a machine that does not answer is a line of the log, not a
//! dialog.

use egui::Context;
use horizon_core::{PanelId, SessionKill, WorkspaceId};

use crate::theme;

use super::HorizonApp;
use super::util::ask_modal;

/// Draws the question that a close puts to the person: the line that says what
/// ends, the red button that ends it and `Cancel`. `Some(true)` answers the red
/// button, `Some(false)` answers `Cancel`, the escape key or a press beside the
/// question, and `None` is a person who owes no answer yet.
fn ask_close(ctx: &Context, id: egui::Id, line: &str, red: &str) -> Option<bool> {
    let mut answer = None;
    let shown = ask_modal(id).show(ctx, |ui| {
        ui.label(egui::RichText::new(line).size(12.5).color(theme::FG()));
        ui.horizontal(|ui| {
            let buttons = [(red, true, theme::PALETTE_RED()), ("Cancel", false, theme::FG_SOFT())];
            for (label, closes, tone) in buttons {
                let button = ui.add(egui::Button::new(egui::RichText::new(label).color(tone)));
                let button = if closes {
                    button.on_hover_text("The work in the tmux sessions of this panel set ends with it.")
                } else {
                    button
                };
                if button.clicked() {
                    answer = Some(closes);
                }
            }
        });
    });
    // A press outside the question, or the escape key, is the `Cancel`.
    answer.or_else(|| shown.should_close().then_some(false))
}

impl HorizonApp {
    /// Closes the panel that a person asked to close: at once for a panel that
    /// holds no session of a machine, and behind the ask for one that does.
    pub(in crate::app) fn request_close_panel(&mut self, panel_id: PanelId) {
        if self.close_ends_a_machine_session(panel_id) {
            self.close_panel_confirm = Some(panel_id);
            return;
        }
        self.close_panel_and_its_caches(panel_id);
    }

    /// Answers the ask of one close: the panel and its session end, or the panel
    /// stays where it is.
    pub(in crate::app) fn answer_close_panel(&mut self, panel_id: PanelId, closes: bool) {
        self.close_panel_confirm = None;
        if closes {
            self.close_panel_and_its_caches(panel_id);
        }
    }

    /// Whether the close of `panel_id` would end a tmux session of a machine.
    fn close_ends_a_machine_session(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .is_some_and(|panel| panel.machine_session().is_some())
    }

    /// The ask of one close, over the panel that it would remove: made only while
    /// the person owes an answer, so no other frame pays for it.
    pub(in crate::app) fn show_close_panel_confirm(&mut self, ctx: &Context) {
        // A panel that left on its own leaves nobody who owes an answer.
        let Some(panel_id) = self.close_panel_confirm.filter(|id| self.board.panel(*id).is_some()) else {
            self.close_panel_confirm = None;
            return;
        };
        let machine = self
            .board
            .panel(panel_id)
            .and_then(|panel| panel.machine().map(str::to_owned))
            .unwrap_or_else(|| "its machine".to_owned());
        let line = format!("End the tmux session of this panel on {machine}?");
        if let Some(closes) = ask_close(ctx, egui::Id::unique("close_panel_confirm"), &line, "Close") {
            self.answer_close_panel(panel_id, closes);
        }
    }

    /// The ask of a whole workspace, over the board that it would empty: made only
    /// while the person owes an answer, so no other frame pays for it.
    pub(in crate::app) fn show_close_workspace_confirm(&mut self, ctx: &Context) {
        // A workspace that left on its own, or one whose sessions ended first,
        // leaves nobody who owes an answer.
        let owed = self
            .close_workspace_confirm
            .map(|workspace_id| (workspace_id, self.board.workspace_sessions(workspace_id).len()))
            .filter(|(_, sessions)| *sessions > 0);
        let Some((workspace_id, sessions)) = owed else {
            self.close_workspace_confirm = None;
            return;
        };
        let line = format!("End {sessions} tmux session(s) of this workspace's machine?");
        if let Some(closes) = ask_close(
            ctx,
            egui::Id::unique("close_workspace_confirm"),
            &line,
            "Close All Panels",
        ) {
            self.answer_close_workspace(workspace_id, closes);
        }
    }

    /// Answers the ask of a whole workspace: its panels end with their sessions,
    /// or every panel stays where it is.
    fn answer_close_workspace(&mut self, workspace_id: WorkspaceId, closes: bool) {
        self.close_workspace_confirm = None;
        if closes {
            self.close_workspace_panels(workspace_id);
        }
    }

    /// Closes `panel_id` now, with the caches that held its view: what the
    /// person's close does at once for a local panel, and after the ask.
    fn close_panel_and_its_caches(&mut self, panel_id: PanelId) {
        self.close_panel(panel_id);
        self.panel_screen_rects.remove(&panel_id);
        self.terminal_body_screen_rects.remove(&panel_id);
        self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
        self.panel_render_caches.editor_preview_cache.remove(&panel_id);
        self.panel_render_caches.browser_ui_state.remove(&panel_id);
        self.panel_render_caches.device_ui_state.remove(&panel_id);
        if self.renaming_panel == Some(panel_id) {
            self.clear_panel_rename();
        }
    }

    /// Ask for the end of the tmux sessions of `panels`, which a close removed.
    pub(in crate::app) fn end_machine_sessions(&mut self, panels: impl IntoIterator<Item = PanelId>) {
        let kills = panels
            .into_iter()
            .filter_map(|panel_id| self.board.panel(panel_id))
            .filter_map(horizon_core::Panel::session_kill)
            .collect::<Vec<_>>();
        self.session_kills.extend(kills);
    }

    /// Ask for the end of the session of one panel that a close removed.
    pub(in crate::app) fn end_machine_session(&mut self, panel_id: PanelId) {
        self.end_machine_sessions([panel_id]);
    }

    /// Hand the kills that closes asked for to the coordination worker. A test
    /// reads [`Self::session_kills`] instead, so it starts no ssh.
    pub(in crate::app) fn dispatch_session_kills(&mut self) {
        for kill in std::mem::take(&mut self.session_kills) {
            self.browser_create_host.io.write(move || SessionKill::run(&kill));
        }
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelId, RuntimeState, WorkspaceId};

    use crate::app::test_support::test_app;

    #[test]
    fn a_board_that_goes_takes_its_unanswered_close_questions_with_it() {
        let (_temp, mut app) = test_app();
        let workspace: WorkspaceId = app.board.create_workspace("work");
        app.close_panel_confirm = Some(PanelId(1));
        app.close_workspace_confirm = Some(workspace);

        app.activate_ephemeral_session(&RuntimeState::default());

        // A panel id and a workspace id name an item of the board that stands
        // now, so a question of the board before would ask about another panel.
        assert_eq!(
            app.close_panel_confirm, None,
            "no question names a panel of the board before"
        );
        assert_eq!(
            app.close_workspace_confirm, None,
            "no question names a workspace of the board before"
        );
    }
}
