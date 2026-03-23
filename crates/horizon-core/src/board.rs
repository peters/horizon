mod arrangement;
mod attention;
mod geometry;
mod shutdown;
mod workspaces;

pub use shutdown::ShutdownProgress;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::attention::AttentionItem;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::panel::{Panel, PanelId, PanelProcessOutput};
use crate::runtime_state::RuntimeState;
use crate::workspace::{Workspace, WorkspaceId};

const PANEL_CHROME_PAD: f32 = 8.0;
const PANEL_CHROME_TITLEBAR: f32 = 34.0;
const TERMINAL_PANEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const READY_FOR_INPUT_AUTO_DISMISS_AFTER: Duration = Duration::from_secs(45);
fn vec2_eq(left: [f32; 2], right: [f32; 2]) -> bool {
    (left[0] - right[0]).abs() <= f32::EPSILON && (left[1] - right[1]).abs() <= f32::EPSILON
}

/// Predefined layout arrangements for panels inside a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceLayout {
    /// Single column, panels stacked top-to-bottom.
    Rows,
    /// Single row, panels side by side.
    Columns,
    /// Square-ish grid (auto columns).
    Grid,
}

impl WorkspaceLayout {
    pub const ALL: [Self; 3] = [Self::Rows, Self::Columns, Self::Grid];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
        }
    }
}

pub struct Board {
    pub panels: Vec<Panel>,
    pub workspaces: Vec<Workspace>,
    pub attention: Vec<AttentionItem>,
    panel_attention_signals: HashMap<PanelId, String>,
    retained_empty_workspaces: HashSet<WorkspaceId>,
    pub focused: Option<PanelId>,
    pub active_workspace: Option<WorkspaceId>,
    pub attention_enabled: bool,
    next_panel_id: u64,
    next_workspace_id: u64,
    next_attention_id: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardProcessOutput {
    pub had_terminal_output: bool,
    pub cwd_changed: bool,
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            panels: Vec::new(),
            workspaces: Vec::new(),
            attention: Vec::new(),
            panel_attention_signals: HashMap::new(),
            retained_empty_workspaces: HashSet::new(),
            focused: None,
            active_workspace: None,
            attention_enabled: false,
            next_panel_id: 1,
            next_workspace_id: 1,
            next_attention_id: 1,
        }
    }

    /// Build a board from a YAML config.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::from_runtime_state(&RuntimeState::from_config(config))
    }

    /// Build a board from a persisted runtime state snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_runtime_state(state: &RuntimeState) -> Result<Self> {
        Self::from_runtime_state_with_transcripts(state, None)
    }

    /// Build a board from a persisted runtime state snapshot and optional transcript root.
    ///
    /// # Errors
    ///
    /// Returns an error if any configured panel fails to spawn.
    pub fn from_runtime_state_with_transcripts(state: &RuntimeState, transcript_root: Option<&Path>) -> Result<Self> {
        let mut board = Self::new();

        for workspace_state in &state.workspaces {
            let ws_id = board.create_workspace_record(workspace_state);
            for panel_state in &workspace_state.panels {
                let mut options = panel_state.to_panel_options();
                options.transcript_root = transcript_root.map(Path::to_path_buf);
                board.create_panel(options, ws_id)?;
            }
        }

        if let Some(local_id) = &state.active_workspace_local_id
            && let Some(workspace_id) = board.workspace_id_by_local_id(local_id)
        {
            board.active_workspace = Some(workspace_id);
        }

        if let Some(local_id) = &state.focused_panel_local_id
            && let Some(panel_id) = board.panel_id_by_local_id(local_id)
        {
            board.focused = Some(panel_id);
            board.active_workspace = board.panel_workspace_id(panel_id);
        } else {
            board.focused = board.panels.first().map(|panel| panel.id);
        }

        Ok(board)
    }

    /// Restart a panel's terminal process in-place, preserving identity and
    /// session binding.
    ///
    /// # Errors
    ///
    /// Returns an error if the new terminal cannot be spawned.
    pub fn restart_panel(&mut self, id: PanelId) -> Result<()> {
        let panel = self
            .panels
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::Pty(format!("panel {} not found", id.0)))?;
        panel.restart()
    }

    pub fn shutdown_terminal_panels(&mut self) {
        for panel in &mut self.panels {
            if panel.terminal().is_some() {
                panel.request_shutdown();
            }
        }

        for panel in &mut self.panels {
            if panel.terminal().is_none() {
                continue;
            }
            if !panel.wait_for_shutdown(TERMINAL_PANEL_SHUTDOWN_TIMEOUT) {
                tracing::warn!(
                    panel_id = panel.id.0,
                    kind = ?panel.kind,
                    timeout_ms = TERMINAL_PANEL_SHUTDOWN_TIMEOUT.as_millis(),
                    "timed out waiting for terminal panel shutdown"
                );
            }
        }
    }

    /// Begins shutting down all terminal panels asynchronously.
    ///
    /// Sends shutdown signals to every terminal and spawns background threads
    /// to join their event loops. Returns a [`ShutdownProgress`] handle that
    /// can be polled each frame to track completion without blocking the UI.
    pub fn begin_async_shutdown(&mut self) -> ShutdownProgress {
        let completed = Arc::new(AtomicUsize::new(0));
        let mut terminal_count = 0;

        for panel in &mut self.panels {
            if let Some(terminal) = panel.terminal_mut() {
                terminal.request_shutdown();
            }
        }

        for panel in &mut self.panels {
            if let Some(terminal) = panel.terminal_mut()
                && terminal.begin_async_join(&completed)
            {
                terminal_count += 1;
            }
        }

        ShutdownProgress::new(terminal_count, completed)
    }

    /// Drain pending output from all panels. Returns `true` if any panel had activity.
    #[profiling::function]
    pub fn process_output(&mut self) -> BoardProcessOutput {
        let mut output = BoardProcessOutput::default();
        for panel in &mut self.panels {
            let panel_output: PanelProcessOutput = panel.process_output();
            output.had_terminal_output |= panel_output.had_output;
            output.cwd_changed |= panel_output.cwd_changed;
        }
        // Only run attention detection when terminals actually produced new
        // output.  The expensive path — `detect_attention()` — locks the
        // terminal mutex and iterates the full display, so skipping it on
        // idle frames is a significant CPU win.
        if self.attention_enabled && output.had_terminal_output {
            self.update_attention();
        }
        output
    }

    /// Returns IDs of panels whose child process has exited.
    #[must_use]
    pub fn exited_panels(&self) -> Vec<PanelId> {
        self.panels
            .iter()
            .filter(|panel| panel.child_exited() && panel.should_close_after_exit())
            .map(|panel| panel.id)
            .collect()
    }

    pub fn focus(&mut self, id: PanelId) {
        self.focused = Some(id);
        self.active_workspace = self.panel_workspace_id(id);
    }

    pub fn focus_workspace(&mut self, id: WorkspaceId) {
        self.active_workspace = Some(id);
        if let Some(workspace) = self.workspace(id)
            && let Some(&panel_id) = workspace.panels.last()
        {
            self.focused = Some(panel_id);
        }
    }

    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|workspace| workspace.id == id)
    }

    #[must_use]
    pub fn panel(&self, id: PanelId) -> Option<&Panel> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn panel_mut(&mut self, id: PanelId) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|panel| panel.id == id)
    }

    #[must_use]
    pub fn panel_workspace_id(&self, id: PanelId) -> Option<WorkspaceId> {
        self.panel(id).map(|panel| panel.workspace_id)
    }

    #[must_use]
    pub fn workspace_for_panel(&self, id: PanelId) -> Option<&Workspace> {
        self.panel_workspace_id(id)
            .and_then(|workspace_id| self.workspace(workspace_id))
    }

    #[must_use]
    pub fn workspace_id_by_local_id(&self, local_id: &str) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.local_id == local_id)
            .map(|workspace| workspace.id)
    }

    #[must_use]
    pub fn panel_id_by_local_id(&self, local_id: &str) -> Option<PanelId> {
        self.panels
            .iter()
            .find(|panel| panel.local_id == local_id)
            .map(|panel| panel.id)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
