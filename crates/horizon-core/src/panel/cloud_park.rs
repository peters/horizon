//! Parking of cloud members. A parked member has no SSH client, PTY or terminal grid;
//! it keeps only the text of its last screen. Its agent continues in tmux on the worker.
use super::{CloudWait, Panel, spawn};
use crate::{agents::AgentStatus, editor::PanelContent, error::Result};

/// The last screen of a parked member, as passive text, and the size of the terminal
/// that showed it, so the member attaches again at the same size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParkedScreen {
    lines: Vec<String>,
    rows: u16,
    cols: u16,
}

impl ParkedScreen {
    /// The screen `lines` of a terminal with `rows` and `cols`.
    #[must_use]
    pub fn new(lines: Vec<String>, rows: u16, cols: u16) -> Self {
        Self { lines, rows, cols }
    }

    /// The text of each row of the screen, top to bottom.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The rows and columns of the terminal that showed the screen.
    #[must_use]
    pub const fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }
}

impl Panel {
    /// Parks this member of a cloud. A running terminal is replaced by the text of its
    /// last screen, and the terminal is shut down, which detaches its tmux client. A
    /// placeholder that waits for another reason says that it is parked instead.
    /// Returns whether the panel changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the placeholder terminal cannot be created.
    pub fn park_cloud(&mut self) -> Result<bool> {
        match self.cloud_wait {
            Some(CloudWait::Parked) => return Ok(false),
            Some(_) => return Ok(self.show_parked_placeholder()),
            None => {}
        }
        let Some(terminal) = self.terminal() else {
            return Ok(false);
        };
        let screen = ParkedScreen::new(terminal.viewport_text(), terminal.rows(), terminal.cols());
        if let PanelContent::Terminal(mut old) = std::mem::replace(&mut self.content, PanelContent::Parked(screen)) {
            old.request_shutdown();
        }
        self.cloud_wait = Some(CloudWait::Parked);
        Ok(true)
    }

    /// Makes a placeholder say, as passive text, that its panel is parked. It keeps the
    /// size of the placeholder's terminal, which shuts down. Returns whether it changed.
    pub(crate) fn show_parked_placeholder(&mut self) -> bool {
        let Some((rows, cols)) = self.terminal().map(|terminal| (terminal.rows(), terminal.cols())) else {
            return false;
        };
        let lines = spawn::placeholder_lines(&self.title, spawn::Placeholder::Cloud(CloudWait::Parked));
        let screen = ParkedScreen::new(lines, rows, cols);
        if let PanelContent::Terminal(mut old) = std::mem::replace(&mut self.content, PanelContent::Parked(screen)) {
            old.request_shutdown();
        }
        self.cloud_wait = Some(CloudWait::Parked);
        true
    }

    /// The last screen of a parked member.
    #[must_use]
    pub fn parked_screen(&self) -> Option<&ParkedScreen> {
        match &self.content {
            PanelContent::Parked(screen) => Some(screen),
            _ => None,
        }
    }

    /// Shows the agent status that the worker reports for a parked member. Every
    /// other panel keeps the status read from its own screen.
    pub fn set_parked_agent_status(&mut self, status: AgentStatus) {
        if self.cloud_wait == Some(CloudWait::Parked) {
            self.agent_status = status;
        }
    }
}
