//! The tmux sessions that a close ended, and the worker that ends them.
//!
//! A terminal of a workspace that is placed on a machine names a tmux session on
//! that machine. Closing the panel by the person ends that session, so the work
//! of a closed panel does not fill the machine; quitting Horizon ends none, so
//! the sessions outlive the window.
//!
//! The close asks for the kill and the next frame hands it to the coordination
//! worker, which runs `ssh` and waits at most ten seconds. A close never waits
//! for a machine, and a machine that does not answer is a line of the log, not a
//! dialog.

use horizon_core::{PanelId, SessionKill};

use super::HorizonApp;

impl HorizonApp {
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
