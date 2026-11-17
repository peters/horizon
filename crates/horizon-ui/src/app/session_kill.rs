//! The ask before a close ends a tmux session, and the worker that ends it.
//!
//! A terminal of a workspace that is placed on a machine names a tmux session on
//! that machine. Closing the panel by the person ends that session, so the work
//! of a closed panel does not fill the machine; quitting Horizon ends none, so
//! the sessions outlive the window.
//!
//! The close of one panel asks first, in the words of the close of a whole
//! workspace. A panel that holds no session of a machine closes at once.
//!
//! The close asks for the kill and the next frame hands it to the coordination
//! worker, which runs `ssh` and waits at most ten seconds. A close never waits
//! for a machine, and a machine that does not answer is a line of the log, not a
//! dialog.

use egui::Context;
use horizon_core::{PanelId, SessionKill};

use crate::theme;

use super::HorizonApp;

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
        let mut answer = None;
        let shown = egui::Modal::new(egui::Id::unique("close_panel_confirm")).show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("End the tmux session of this panel on {machine}?"))
                    .size(12.5)
                    .color(theme::FG()),
            );
            ui.horizontal(|ui| {
                let buttons = [
                    ("Close", true, theme::PALETTE_RED()),
                    ("Cancel", false, theme::FG_SOFT()),
                ];
                for (label, closes, tone) in buttons {
                    if ui
                        .add(egui::Button::new(egui::RichText::new(label).color(tone)))
                        .clicked()
                    {
                        answer = Some(closes);
                    }
                }
            });
        });
        // A press outside the ask, or the escape key, is the `Cancel`.
        if let Some(closes) = answer.or_else(|| shown.should_close().then_some(false)) {
            self.answer_close_panel(panel_id, closes);
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
