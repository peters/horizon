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
use crate::panel::{Panel, PanelId};
use crate::runtime_state::RuntimeState;
use crate::workspace::{Workspace, WorkspaceId};

const PANEL_CHROME_PAD: f32 = 8.0;
const PANEL_CHROME_TITLEBAR: f32 = 34.0;
const TERMINAL_PANEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const READY_FOR_INPUT_AUTO_DISMISS_AFTER: Duration = Duration::from_secs(45);
const STACK_OFFSET_X: f32 = 16.0;
const STACK_OFFSET_Y: f32 = 20.0;
const CASCADE_OFFSET_X: f32 = 40.0;
const CASCADE_OFFSET_Y: f32 = 30.0;

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
    /// Layered pile with slight offsets to keep nearby panels accessible.
    Stack,
    /// Diagonal overlap that fans panels across the workspace.
    Cascade,
}

impl WorkspaceLayout {
    pub const ALL: [Self; 5] = [Self::Rows, Self::Columns, Self::Grid, Self::Stack, Self::Cascade];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
            Self::Stack => "Stack",
            Self::Cascade => "Cascade",
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
    pub fn process_output(&mut self) -> bool {
        let mut had_output = false;
        for panel in &mut self.panels {
            had_output |= panel.process_output();
        }
        // Only run attention detection when terminals actually produced new
        // output.  The expensive path — `detect_attention()` — locks the
        // terminal mutex and iterates the full display, so skipping it on
        // idle frames is a significant CPU win.
        if self.attention_enabled && had_output {
            self.update_attention();
        }
        had_output
    }

    /// Returns IDs of panels whose child process has exited.
    #[must_use]
    pub fn exited_panels(&self) -> Vec<PanelId> {
        self.panels
            .iter()
            .filter(|panel| panel.child_exited())
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
mod tests {
    use super::*;
    use crate::attention::{AttentionSeverity, AttentionState};
    use crate::config::WorkspaceConfig;
    use crate::layout::{TILE_GAP, WS_INNER_PAD};
    use crate::panel::{DEFAULT_PANEL_SIZE, PanelKind, PanelOptions};
    use crate::runtime_state::WorkspaceTemplateRef;
    use std::path::PathBuf;

    #[test]
    fn rename_workspace_updates_matching_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");

        assert!(board.rename_workspace(workspace_id, "backend"));
        assert_eq!(board.workspaces[0].name, "backend");
    }

    #[test]
    fn rename_workspace_rejects_blank_names() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");

        assert!(!board.rename_workspace(workspace_id, "   "));
        assert_eq!(board.workspaces[0].name, "frontend");
    }

    #[test]
    fn rename_panel_updates_matching_panel() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        assert!(board.rename_panel(panel_id, "backend shell"));
        assert_eq!(
            board.panel(panel_id).expect("panel should exist").title,
            "backend shell"
        );
    }

    #[test]
    fn rename_panel_rejects_blank_names() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        let original_title = board.panel(panel_id).expect("panel should exist").title.clone();

        assert!(!board.rename_panel(panel_id, "   "));
        assert_eq!(board.panel(panel_id).expect("panel should exist").title, original_title);
    }

    #[test]
    fn close_panel_removes_panel_attention() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = PanelId(7);

        board.create_attention(
            workspace_id,
            Some(panel_id),
            "codex-ui",
            "Needs user feedback",
            AttentionSeverity::High,
        );

        board.close_panel(panel_id);

        assert!(board.unresolved_attention().next().is_none());
    }

    #[test]
    fn resolve_attention_marks_item_resolved() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let attention_id = board.create_attention(
            workspace_id,
            None,
            "system",
            "Review build result",
            AttentionSeverity::Medium,
        );

        assert!(board.resolve_attention(attention_id));
        assert!(board.unresolved_attention().next().is_none());
    }

    #[test]
    fn dismissing_attention_keeps_same_signal_suppressed_until_it_clears() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("agents");
        let panel_id = PanelId(99);

        board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
        let attention_id = board.unresolved_attention().next().expect("open attention").id;
        assert!(board.dismiss_attention(attention_id));

        board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
        assert!(board.unresolved_attention().next().is_none());

        board.reconcile_agent_attention_signal(panel_id, workspace_id, "");
        board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
        assert!(board.unresolved_attention().next().is_some());
    }

    #[test]
    fn stale_ready_for_input_attention_auto_dismisses() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("agents");
        let attention_id = board.create_attention(
            workspace_id,
            Some(PanelId(7)),
            "agent",
            "Ready for input",
            AttentionSeverity::High,
        );

        let item = board
            .attention
            .iter_mut()
            .find(|item| item.id == attention_id)
            .expect("attention item");
        item.created_at = std::time::SystemTime::now() - READY_FOR_INPUT_AUTO_DISMISS_AFTER - Duration::from_secs(1);

        board.dismiss_expired_ready_attention(READY_FOR_INPUT_AUTO_DISMISS_AFTER);

        let item = board
            .attention
            .iter()
            .find(|item| item.id == attention_id)
            .expect("attention item");
        assert_eq!(item.state, AttentionState::Dismissed);
    }

    #[test]
    fn focusing_panel_tracks_active_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        board.focus(panel_id);

        assert_eq!(board.active_workspace, Some(workspace_id));
    }

    #[test]
    fn panels_tile_within_workspace_region() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let ws_pos = board.workspace(workspace_id).unwrap().position;

        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        let panel = board.panel(panel_id).expect("panel should exist");

        // First panel tiles at workspace origin + inner padding.
        assert!((panel.layout.position[0] - (ws_pos[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[1] - (ws_pos[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
    }

    #[test]
    fn workspaces_are_placed_apart() {
        let mut board = Board::new();
        let ws1 = board.create_workspace("first");
        board
            .create_panel(PanelOptions::default(), ws1)
            .expect("panel should spawn");
        let ws2 = board.create_workspace("second");

        let pos1 = board.workspace(ws1).unwrap().position;
        let pos2 = board.workspace(ws2).unwrap().position;

        // Second workspace must start to the right of the first.
        assert!(pos2[0] > pos1[0] + DEFAULT_PANEL_SIZE[0]);
    }

    #[test]
    fn assign_panel_moves_it_to_target_workspace() {
        let mut board = Board::new();
        let ws1 = board.create_workspace("source");
        let ws2 = board.create_workspace("target");

        let panel_id = board
            .create_panel(PanelOptions::default(), ws1)
            .expect("panel should spawn");

        let ws2_pos = board.workspace(ws2).unwrap().position;
        board.assign_panel_to_workspace(panel_id, ws2);

        let panel = board.panel(panel_id).unwrap();
        assert_eq!(panel.workspace_id, ws2);
        // Panel should be within the target workspace's region.
        assert!(panel.layout.position[0] >= ws2_pos[0]);
    }

    #[test]
    fn translating_workspace_moves_workspace_origin_and_panels() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("frontend");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        let original_workspace_pos = board.workspace(workspace_id).expect("workspace").position;
        let original_panel_pos = board.panel(panel_id).expect("panel").layout.position;

        assert!(board.translate_workspace(workspace_id, [48.0, 24.0]));

        let workspace = board.workspace(workspace_id).expect("workspace");
        let panel = board.panel(panel_id).expect("panel");
        assert!((workspace.position[0] - (original_workspace_pos[0] + 48.0)).abs() <= f32::EPSILON);
        assert!((workspace.position[1] - (original_workspace_pos[1] + 24.0)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[0] - (original_panel_pos[0] + 48.0)).abs() <= f32::EPSILON);
        assert!((panel.layout.position[1] - (original_panel_pos[1] + 24.0)).abs() <= f32::EPSILON);
    }

    #[test]
    fn sync_workspace_metadata_updates_only_templated_workspaces() {
        let mut board = Board::new();
        let templated_workspace = board.create_workspace("stale");
        let manual_workspace = board.create_workspace("manual");

        {
            let workspace = board.workspace_mut(templated_workspace).expect("templated workspace");
            workspace.template = Some(WorkspaceTemplateRef {
                workspace_index: 0,
                workspace_name: "template".to_string(),
            });
            workspace.cwd = Some(PathBuf::from("/tmp/old"));
        }
        {
            let workspace = board.workspace_mut(manual_workspace).expect("manual workspace");
            workspace.cwd = Some(PathBuf::from("/tmp/manual"));
        }

        let config = Config {
            workspaces: vec![WorkspaceConfig {
                name: "synced".to_string(),
                color: None,
                cwd: Some("~/repo".to_string()),
                position: None,
                terminals: Vec::new(),
            }],
            ..Config::default()
        };

        board.sync_workspace_metadata(&config);

        let templated = board.workspace(templated_workspace).expect("templated workspace");
        assert_eq!(templated.name, "synced");
        assert_eq!(templated.cwd, Some(Config::expand_tilde("~/repo")));

        let manual = board.workspace(manual_workspace).expect("manual workspace");
        assert_eq!(manual.name, "manual");
        assert_eq!(manual.cwd, Some(PathBuf::from("/tmp/manual")));
    }

    /// Regression: when adding a panel with an explicit position (e.g. from a
    /// click on a panned canvas), the panel must be placed at that position
    /// instead of falling back to the default tiled slot.
    #[test]
    fn explicit_position_overrides_default_tiling() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("test");

        // Create a first panel so the workspace is non-empty.
        board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");

        // Add a second panel with an explicit canvas position far from the
        // workspace origin — simulating a Ctrl+double-click on a panned canvas.
        let click_pos = [800.0, 600.0];
        let panel_id = board
            .create_panel(
                PanelOptions {
                    position: Some(click_pos),
                    ..PanelOptions::default()
                },
                workspace_id,
            )
            .expect("second panel should spawn");

        let panel = board.panel(panel_id).expect("panel should exist");
        assert!(
            (panel.layout.position[0] - click_pos[0]).abs() <= f32::EPSILON
                && (panel.layout.position[1] - click_pos[1]).abs() <= f32::EPSILON,
            "panel should be placed at the explicit click position ({:?}), not at the default tile position ({:?})",
            click_pos,
            panel.layout.position,
        );
    }

    #[test]
    fn arrange_workspace_stack_offsets_panels_in_layers() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("stacked");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");

        board.arrange_workspace(workspace_id, WorkspaceLayout::Stack);

        let first_position = board.panel(first).expect("first panel").layout.position;
        let second_position = board.panel(second).expect("second panel").layout.position;

        assert!((first_position[0] - (origin[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((first_position[1] - (origin[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((second_position[0] - (origin[0] + WS_INNER_PAD + STACK_OFFSET_X)).abs() <= f32::EPSILON);
        assert!((second_position[1] - (origin[1] + WS_INNER_PAD + STACK_OFFSET_Y)).abs() <= f32::EPSILON);
    }

    #[test]
    fn arrange_workspace_cascade_offsets_panels_diagonally() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("cascade");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");

        board.arrange_workspace(workspace_id, WorkspaceLayout::Cascade);

        let first_position = board.panel(first).expect("first panel").layout.position;
        let second_position = board.panel(second).expect("second panel").layout.position;

        assert!((first_position[0] - (origin[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((first_position[1] - (origin[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
        assert!((second_position[0] - (origin[0] + WS_INNER_PAD + CASCADE_OFFSET_X)).abs() <= f32::EPSILON);
        assert!((second_position[1] - (origin[1] + WS_INNER_PAD + CASCADE_OFFSET_Y)).abs() <= f32::EPSILON);
    }

    #[test]
    fn arranging_workspace_records_selected_layout() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");

        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        assert_eq!(
            board.workspace(workspace_id).expect("workspace").layout,
            Some(WorkspaceLayout::Rows)
        );
    }

    #[test]
    fn adding_panel_reflows_arranged_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");

        assert!(vec2_eq(
            board.panel(first).expect("first panel").layout.position,
            [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
        ));
        assert!(vec2_eq(
            board.panel(second).expect("second panel").layout.position,
            [
                origin[0] + WS_INNER_PAD,
                origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
            ]
        ));
    }

    #[test]
    fn closing_panel_reflows_arranged_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        board.close_panel(first);

        assert!(vec2_eq(
            board.panel(second).expect("remaining panel").layout.position,
            [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
        ));
        assert_eq!(
            board.workspace(workspace_id).expect("workspace").layout,
            Some(WorkspaceLayout::Rows)
        );
    }

    #[test]
    fn closing_middle_panel_reflows_arranged_workspace() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        let origin = board.workspace(workspace_id).expect("workspace").position;

        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");
        let third = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("third panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        board.close_panel(second);

        assert!(vec2_eq(
            board.panel(first).expect("first panel").layout.position,
            [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
        ));
        assert!(vec2_eq(
            board.panel(third).expect("third panel").layout.position,
            [
                origin[0] + WS_INNER_PAD,
                origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
            ]
        ));
        assert_eq!(
            board.workspace(workspace_id).expect("workspace").layout,
            Some(WorkspaceLayout::Rows)
        );
    }

    #[test]
    fn close_panels_in_workspace_keeps_workspace_available() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("alpha");
        let other_workspace_id = board.create_workspace("beta");
        let first = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");
        let other_panel = board
            .create_panel(PanelOptions::default(), other_workspace_id)
            .expect("other panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        let closed = board.close_panels_in_workspace(workspace_id);
        board.remove_empty_workspaces();

        assert_eq!(closed, vec![first, second]);
        assert!(board.panel(first).is_none());
        assert!(board.panel(second).is_none());
        assert!(board.panel(other_panel).is_some());
        assert_eq!(board.focused, None);
        assert_eq!(board.active_workspace, Some(workspace_id));
        assert_eq!(
            board.workspace(workspace_id).expect("workspace").layout,
            Some(WorkspaceLayout::Rows)
        );
        assert!(board.workspace(workspace_id).expect("workspace").panels.is_empty());
    }

    #[test]
    fn shutdown_terminal_panels_waits_for_shell_and_command_panels() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("shutdown");
        let shell_panel = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("shell panel should spawn");
        let command_panel = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Command,
                    ..PanelOptions::default()
                },
                workspace_id,
            )
            .expect("command panel should spawn");

        board.shutdown_terminal_panels();

        assert!(
            board
                .panel_mut(shell_panel)
                .expect("shell panel should exist")
                .wait_for_shutdown(Duration::from_millis(10))
        );
        assert!(
            board
                .panel_mut(command_panel)
                .expect("command panel should exist")
                .wait_for_shutdown(Duration::from_millis(10))
        );
    }

    #[test]
    fn manual_panel_move_returns_workspace_to_freeform() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        assert!(board.move_panel(panel_id, [420.0, 360.0]));

        assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
    }

    #[test]
    fn clearing_workspace_layout_preserves_current_panel_positions() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("rows");
        let panel_id = board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

        let arranged_position = board.panel(panel_id).expect("panel").layout.position;

        assert!(board.clear_workspace_layout(workspace_id));

        assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
        let current_position = board.panel(panel_id).expect("panel").layout.position;
        assert!(
            vec2_eq(current_position, arranged_position),
            "expected {arranged_position:?}, got {current_position:?}"
        );
    }

    #[test]
    fn resize_panel_pushes_sibling() {
        let mut board = Board::new();
        let ws_id = board.create_workspace("test");

        // Panel A at (0,0), size 200x200.
        let a = board
            .create_panel(
                PanelOptions {
                    position: Some([0.0, 0.0]),
                    size: Some([200.0, 200.0]),
                    ..PanelOptions::default()
                },
                ws_id,
            )
            .expect("panel A should spawn");

        // Panel B at (220,0), size 200x200 — TILE_GAP gap.
        let b = board
            .create_panel(
                PanelOptions {
                    position: Some([220.0, 0.0]),
                    size: Some([200.0, 200.0]),
                    ..PanelOptions::default()
                },
                ws_id,
            )
            .expect("panel B should spawn");

        // Resize A to 300x200 — it now overlaps B's position.
        board.resize_panel(a, [300.0, 200.0]);

        // B should have been pushed right.
        let b_pos = board.panel(b).expect("panel B").layout.position;
        assert!(
            b_pos[0] >= 300.0 + TILE_GAP - 1.0,
            "panel B should be pushed right, got x={}",
            b_pos[0],
        );
    }
}
