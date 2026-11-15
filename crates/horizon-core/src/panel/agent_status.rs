//! The working indicator of the screen of a panel, and the claims of its program.
//!
//! The terminal screen is the rule for every agent panel whose launch reports
//! nothing, and for a panel of a kind without a state title or a program that
//! speaks the program status protocol: an agent TUI pins a spinner and a working
//! status line near the bottom of its screen while it works, and
//! `Panel::detect_agent_status` reads that line.
//!
//! The other two sources are the program's own statement about its panel: the
//! title of its terminal, and the report of the program status protocol that the
//! reader of the panel's PTY hands over. `Panel::read_program_claims` reads both
//! when they change.

use crate::agent_work::{ProgramStatus, TitleState};
use crate::agents::{AGENT_WORKING_SCAN_ROWS, AgentStatus, is_agent_working_line};

use super::Panel;

impl Panel {
    /// Check whether this panel's agent TUI is rendering its working
    /// indicator (animated spinner plus a working status line pinned near the
    /// bottom of the screen, as Pi, Codex, Claude Code & co. do while busy).
    #[must_use]
    pub fn detect_agent_status(&self) -> AgentStatus {
        if !self.kind.is_agent() {
            return AgentStatus::Idle;
        }
        let Some(terminal) = self.content.terminal() else {
            return AgentStatus::Idle;
        };
        if terminal
            .bottom_lines_text(AGENT_WORKING_SCAN_ROWS)
            .iter()
            .any(|line| is_agent_working_line(line))
        {
            AgentStatus::Working
        } else {
            AgentStatus::Idle
        }
    }

    /// Reads the state that the program of this panel claims about its panel.
    ///
    /// A title is a value that the VT parser hands to the panel, and a report of
    /// the program status protocol is a value that the reader of the PTY keeps
    /// beside it, because the parser drops a private OSC number. Both are read when
    /// they change, and neither costs work on a frame of a program that repeats the
    /// state it already reported.
    pub(crate) fn read_program_claims(&mut self) {
        let Some(terminal) = self.content.terminal() else {
            return;
        };
        if self.terminal_title != terminal.title() {
            self.terminal_title = terminal.title().to_string();
            self.title_state = crate::agent_work::title_state(self.kind, &self.terminal_title);
        }
        if let Some(payload) = terminal.take_program_payload() {
            self.program_status = crate::agent_work::program_status(self.kind, payload.as_bytes());
        }
    }

    /// The state that this panel's own title claims, for an agent kind that
    /// writes its state into the title of its terminal.
    #[must_use]
    pub const fn title_state(&self) -> TitleState {
        self.title_state
    }

    /// The newest state that the program of this launch reported about the panel,
    /// for an agent kind whose program speaks the program status protocol.
    #[must_use]
    pub const fn program_status(&self) -> ProgramStatus {
        self.program_status
    }
}
