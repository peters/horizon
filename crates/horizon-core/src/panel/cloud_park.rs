//! Parking of cloud members. A parked member has no SSH client or PTY; it keeps
//! only a grid with its last screen. Its agent continues in tmux on the worker.
use super::{CloudWait, Panel, spawn};
use crate::{agents::AgentStatus, editor::PanelContent, error::Result};

impl Panel {
    /// Parks this member of a cloud. A running terminal is replaced by a placeholder
    /// that shows its last screen, and the old terminal is shut down, which detaches
    /// its tmux client. A placeholder that waits for another reason says that it is
    /// parked instead. Returns whether the panel changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the placeholder terminal cannot be created.
    pub fn park_cloud(&mut self) -> Result<bool> {
        match self.cloud_wait {
            Some(CloudWait::Parked) => return Ok(false),
            Some(_) => return self.show_cloud_wait(CloudWait::Parked),
            None => {}
        }
        let Some(terminal) = self.terminal() else {
            return Ok(false);
        };
        let (rows, cols) = (terminal.rows(), terminal.cols());
        let screen = terminal.viewport_text();
        let replacement = spawn::parked_terminal(self, rows, cols, &screen)?;
        if let PanelContent::Terminal(mut old) =
            std::mem::replace(&mut self.content, PanelContent::Terminal(replacement))
        {
            old.request_shutdown();
        }
        self.cloud_wait = Some(CloudWait::Parked);
        Ok(true)
    }

    /// Shows the agent status that the worker reports for a parked member. Every
    /// other panel keeps the status read from its own screen.
    pub fn set_parked_agent_status(&mut self, status: AgentStatus) {
        if self.cloud_wait == Some(CloudWait::Parked) {
            self.agent_status = status;
        }
    }
}
