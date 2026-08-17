use std::time::Duration;

use crate::agents::AgentStatus;
use crate::panel::Panel;

use super::Board;

/// A working indicator that receives no terminal output for this long is
/// treated as stale: the agent finished without a final repaint, hung, or the
/// TUI stopped redrawing.
const WORKING_STALE_AFTER: Duration = Duration::from_secs(2);

impl Board {
    /// Refresh each agent panel's working status.
    ///
    /// Runs every frame, but only panels that received new terminal output
    /// this frame pay for the screen scan; quiet panels keep their status
    /// until a working flag goes stale.
    pub(super) fn update_agent_status(&mut self) {
        for panel in self.panels.iter_mut().filter(|panel| panel.kind.is_agent()) {
            update_panel_agent_status(panel, WORKING_STALE_AFTER);
        }
    }
}

fn update_panel_agent_status(panel: &mut Panel, stale_after: Duration) {
    if panel.had_recent_output {
        panel.agent_status = panel.detect_agent_status();
    } else if panel.agent_status == AgentStatus::Working && !panel.had_recent_output_within(stale_after) {
        panel.agent_status = AgentStatus::Idle;
    }
}
