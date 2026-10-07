use std::time::Duration;

use crate::agents::{AgentStatus, WORKING_STALE_AFTER};
use crate::panel::Panel;

use super::Board;

impl Board {
    /// Refresh each agent panel's working status.
    ///
    /// Runs every frame, but only panels that received new terminal output
    /// this frame pay for the screen scan; quiet panels keep their status
    /// until a working flag goes stale.
    pub(super) fn update_agent_status(&mut self) {
        // A parked member shows the status its worker reports, not its placeholder screen.
        for panel in self
            .panels
            .iter_mut()
            .filter(|panel| panel.kind.is_agent() && panel.cloud_wait() != Some(crate::panel::CloudWait::Parked))
        {
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
