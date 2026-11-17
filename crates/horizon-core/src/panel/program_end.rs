//! The end of the program of a terminal panel.
//!
//! A shell, a command or an agent that ends leaves its last screen in the panel
//! and writes nothing more. The person then reads one line under the grid that
//! says what happened, and two controls start the program again or close the
//! panel. Horizon itself never closes a panel and never starts a program again
//! because it ended.
//!
//! Two panels say their own line instead: an SSH panel reports a lost link, and
//! a snapshot panel runs a stand-in process that draws a screen no program
//! produced, so neither reports an ended program here.

use super::{Panel, PanelKind};
use crate::editor::PanelContent;

/// The end of the program of one panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramEnd {
    /// The exit status that the system reported, or `None` when it reported
    /// none. A signalled program reports no code.
    pub exit_status: Option<i32>,
}

impl ProgramEnd {
    /// The sentence for the person, in the form of the lost-link line: what
    /// happened, then that typed text reaches nothing.
    #[must_use]
    pub fn line(&self) -> String {
        let ended = match self.exit_status {
            Some(status) if status != 0 => format!("The program of this panel ended with status {status}."),
            Some(_) | None => "The program of this panel ended.".to_owned(),
        };
        format!("{ended} Text typed here is not sent.")
    }
}

impl Panel {
    /// The end of the program of this panel, when it ended and still has no
    /// program.
    #[must_use]
    pub fn program_end(&self) -> Option<ProgramEnd> {
        self.program_end
    }

    /// Records the end of the program of this panel once, so later frames read
    /// the same status and the panel does not report an end twice.
    pub(super) fn track_program_end(&mut self, child_exited: bool) {
        if !child_exited || self.program_end.is_some() || !self.reports_its_program_end() {
            return;
        }
        let status = self
            .content
            .terminal()
            .and_then(crate::terminal::Terminal::child_exit_status)
            .and_then(|status| status.code());
        self.program_end = Some(ProgramEnd { exit_status: status });
        tracing::info!(panel_id = self.id.0, exit_status = ?status, "the program of a panel ended");
    }

    /// Whether this panel says a line when its program ends: it holds the
    /// program of a shell, a command or an agent, whose end nothing else
    /// reports.
    fn reports_its_program_end(&self) -> bool {
        // A follow panel has no program: its feed says when its stream ended.
        if self.kind == PanelKind::Ssh || self.is_follower() {
            return false;
        }
        match &self.content {
            PanelContent::Terminal(terminal) => !terminal.is_snapshot(),
            PanelContent::Editor(_) | PanelContent::Browser(_) | PanelContent::Device(_) | PanelContent::Block(_) => {
                false
            }
        }
    }

    /// Whether this panel still shows the program that it started. Keys typed
    /// into a panel that does not are dropped, and the line under its grid is
    /// the answer that says so.
    #[must_use]
    pub fn accepts_input(&self) -> bool {
        self.program_end.is_none()
    }

    /// Forgets the end of the program of this panel, because a program starts in
    /// it again.
    pub(super) fn clear_program_end(&mut self) {
        self.program_end = None;
    }
}

#[cfg(all(test, unix))]
mod tests;
