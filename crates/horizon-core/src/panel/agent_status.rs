//! The working indicator of the screen of a panel.
//!
//! The terminal screen is the rule for every agent panel whose launch reports
//! nothing, and for a panel of a kind without a state title: an agent TUI pins a
//! spinner and a working status line near the bottom of its screen while it
//! works, and `Panel::detect_agent_status` reads that line.

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
}
