mod agent_sessions;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::board::{Board, WorkspaceLayout};
use crate::config::{Config, TerminalConfig, WindowConfig, WorkspaceConfig};
use crate::error::{Error, Result};
use crate::layout::workspace_slot_width;
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::ssh::SshConnection;
use crate::terminal::Terminal;
use crate::view::CanvasViewState;

pub use agent_sessions::{AgentSessionCatalog, AgentSessionRecord};

const RUNTIME_STATE_VERSION: u32 = 2;
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const MAX_CLAUDE_SESSION_FILES: usize = 64;
const CLAUDE_SESSION_HEAD_LINE_LIMIT: usize = 48;
const CLAUDE_SESSION_TAIL_LINE_LIMIT: usize = 24;
const CLAUDE_SESSION_TAIL_BYTES: u64 = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RuntimeState {
    pub version: u32,
    pub window: Option<WindowConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_view: Option<CanvasViewState>,
    #[serde(default, skip_serializing)]
    pub pan_offset: Option<[f32; 2]>,
    pub active_workspace_local_id: Option<String>,
    pub focused_panel_local_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached_workspaces: Vec<DetachedWorkspaceState>,
    pub workspaces: Vec<WorkspaceState>,
}

impl RuntimeState {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut next_workspace_x = 0.0;
        let workspaces = config
            .workspaces
            .iter()
            .enumerate()
            .map(|(workspace_index, workspace)| {
                let resolved_position = workspace.position.unwrap_or([next_workspace_x, 40.0]);
                next_workspace_x = next_workspace_x.max(resolved_position[0] + workspace_slot_width());
                WorkspaceState::from_config(workspace_index, workspace, resolved_position)
            })
            .collect();

        Self {
            version: RUNTIME_STATE_VERSION,
            window: None,
            canvas_view: None,
            pan_offset: None,
            active_workspace_local_id: None,
            focused_panel_local_id: None,
            detached_workspaces: Vec::new(),
            workspaces,
        }
    }

    /// Load a persisted runtime state file if it exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the state file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(path)?;
        let mut state = serde_yaml::from_str::<Self>(&content).map_err(|error| Error::State(error.to_string()))?;
        state.ensure_local_ids();
        state.migrate_canvas_view();
        state.version = RUNTIME_STATE_VERSION;
        Ok(Some(state))
    }

    /// Serialize this runtime state to YAML.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|error| Error::State(error.to_string()))
    }

    #[must_use]
    pub fn window_or<'a>(&'a self, fallback: &'a WindowConfig) -> &'a WindowConfig {
        self.window.as_ref().unwrap_or(fallback)
    }

    #[must_use]
    pub fn canvas_view_or_default(&self) -> CanvasViewState {
        self.canvas_view
            .or_else(|| self.pan_offset.map(CanvasViewState::from_legacy_pan_offset))
            .unwrap_or_default()
            .clamped()
    }

    #[must_use]
    pub fn has_persisted_canvas_view(&self) -> bool {
        self.canvas_view.is_some() || self.pan_offset.is_some()
    }

    pub fn ensure_local_ids(&mut self) {
        if self.version == 0 {
            self.version = RUNTIME_STATE_VERSION;
        }

        for workspace in &mut self.workspaces {
            if workspace.local_id.is_empty() {
                workspace.local_id = new_local_id();
            }
            for panel in &mut workspace.panels {
                if panel.local_id.is_empty() {
                    panel.local_id = new_local_id();
                }
            }
        }
    }

    pub fn migrate_canvas_view(&mut self) {
        self.canvas_view = Some(self.canvas_view_or_default());
        self.pan_offset = None;
    }

    pub fn bootstrap_missing_agent_bindings(&mut self, catalog: &AgentSessionCatalog) {
        self.ensure_local_ids();

        let mut used_session_ids = HashSet::new();

        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.is_agent() {
                continue;
            }

            if panel.session_binding.is_none()
                && let PanelResume::Session { session_id } = &panel.resume
            {
                panel.session_binding = Some(AgentSessionBinding::new(
                    panel.kind,
                    session_id.clone(),
                    panel.cwd.clone(),
                    Some(panel.name.clone()),
                    None,
                ));
            }

            if let Some(binding) = &panel.session_binding {
                used_session_ids.insert(binding.session_id.clone());
            }
        }

        let mut pending_by_group: HashMap<(PanelKind, String), Vec<&mut PanelState>> = HashMap::new();
        for panel in self.workspaces.iter_mut().flat_map(|workspace| &mut workspace.panels) {
            if !panel.kind.is_agent() || panel.session_binding.is_some() || !matches!(panel.resume, PanelResume::Last) {
                continue;
            }
            let cwd = normalize_cwd(panel.cwd.as_deref()).unwrap_or_default();
            pending_by_group.entry((panel.kind, cwd)).or_default().push(panel);
        }

        for ((kind, cwd), panels) in pending_by_group {
            let mut candidates = catalog.recent_for(kind, empty_to_none(&cwd));
            candidates.retain(|candidate| !used_session_ids.contains(&candidate.session_id));

            for (panel, candidate) in panels.into_iter().zip(candidates) {
                used_session_ids.insert(candidate.session_id.clone());
                panel.session_binding = Some(candidate.into_binding());
            }
        }
    }

    #[must_use]
    pub fn panel_count(&self) -> usize {
        self.workspaces.iter().map(|workspace| workspace.panels.len()).sum()
    }

    #[must_use]
    pub fn from_board(board: &Board, window: WindowConfig, canvas_view: CanvasViewState) -> Self {
        Self::from_board_with_detached_workspaces(board, window, canvas_view, Vec::new())
    }

    #[must_use]
    pub fn from_board_with_detached_workspaces(
        board: &Board,
        window: WindowConfig,
        canvas_view: CanvasViewState,
        detached_workspaces: Vec<DetachedWorkspaceState>,
    ) -> Self {
        let workspaces = board
            .workspaces
            .iter()
            .map(|workspace| {
                let panels = workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| board.panel(*panel_id))
                    .filter(|panel| panel.kind != PanelKind::RemoteHosts)
                    .map(|panel| {
                        let terminal = panel.terminal();
                        let editor = panel.editor();

                        PanelState {
                            local_id: panel.local_id.clone(),
                            name: panel.title.clone(),
                            kind: panel.kind,
                            command: panel.launch_command.clone(),
                            args: panel.launch_args.clone(),
                            cwd: if panel.kind.is_agent() || panel.kind == PanelKind::Ssh {
                                panel.launch_cwd.clone()
                            } else {
                                terminal
                                    .and_then(Terminal::current_cwd)
                                    .or_else(|| panel.launch_cwd.clone())
                            }
                            .map(|path| path.display().to_string()),
                            rows: terminal.map_or(DEFAULT_ROWS, Terminal::rows),
                            cols: terminal.map_or(DEFAULT_COLS, Terminal::cols),
                            resume: panel.resume.clone(),
                            position: Some(panel.layout.position),
                            size: Some(panel.layout.size),
                            ssh_connection: panel.ssh_connection.clone(),
                            session_binding: panel.session_binding.clone(),
                            template: panel.template.clone(),
                            editor_content: editor
                                .filter(|editor| editor.file_path.is_none() && !editor.text.is_empty())
                                .map(|editor| editor.text.clone()),
                        }
                    })
                    .collect();

                WorkspaceState {
                    local_id: workspace.local_id.clone(),
                    name: workspace.name.clone(),
                    cwd: workspace.cwd.as_ref().map(|path| path.display().to_string()),
                    position: Some(workspace.position),
                    template: workspace.template.clone(),
                    layout: workspace.layout,
                    panels,
                }
            })
            .collect();

        Self {
            version: RUNTIME_STATE_VERSION,
            window: Some(window),
            canvas_view: Some(canvas_view.clamped()),
            pan_offset: None,
            active_workspace_local_id: board
                .active_workspace
                .and_then(|workspace_id| board.workspace(workspace_id))
                .map(|workspace| workspace.local_id.clone()),
            focused_panel_local_id: board
                .focused
                .and_then(|panel_id| board.panel(panel_id))
                .map(|panel| panel.local_id.clone()),
            detached_workspaces,
            workspaces,
        }
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            version: RUNTIME_STATE_VERSION,
            window: None,
            canvas_view: None,
            pan_offset: None,
            active_workspace_local_id: None,
            focused_panel_local_id: None,
            detached_workspaces: Vec::new(),
            workspaces: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DetachedWorkspaceState {
    pub workspace_local_id: String,
    pub window: WindowConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspaceState {
    pub local_id: String,
    pub name: String,
    pub cwd: Option<String>,
    pub position: Option<[f32; 2]>,
    pub template: Option<WorkspaceTemplateRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_workspace_layout"
    )]
    pub layout: Option<WorkspaceLayout>,
    pub panels: Vec<PanelState>,
}

fn deserialize_workspace_layout<'de, D>(deserializer: D) -> std::result::Result<Option<WorkspaceLayout>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    let Some(value) = raw else {
        return Ok(None);
    };

    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rows" => Ok(Some(WorkspaceLayout::Rows)),
        "columns" | "cols" => Ok(Some(WorkspaceLayout::Columns)),
        "grid" => Ok(Some(WorkspaceLayout::Grid)),
        "stack" | "cascade" => Ok(None),
        _ => Err(serde::de::Error::unknown_variant(
            &value,
            &["Rows", "Columns", "Grid", "Stack", "Cascade"],
        )),
    }
}

impl WorkspaceState {
    #[must_use]
    pub fn from_config(workspace_index: usize, workspace: &WorkspaceConfig, resolved_position: [f32; 2]) -> Self {
        let workspace_cwd = normalize_cwd(workspace.cwd.as_deref());
        let panels = workspace
            .terminals
            .iter()
            .enumerate()
            .map(|(panel_index, panel)| {
                PanelState::from_config(
                    workspace_index,
                    &workspace.name,
                    panel_index,
                    workspace,
                    resolved_position,
                    panel,
                )
            })
            .collect();

        Self {
            local_id: new_local_id(),
            name: workspace.name.clone(),
            cwd: workspace_cwd,
            position: Some(resolved_position),
            template: Some(WorkspaceTemplateRef {
                workspace_index,
                workspace_name: workspace.name.clone(),
            }),
            layout: None,
            panels,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PanelState {
    pub local_id: String,
    pub name: String,
    pub kind: PanelKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
    pub rows: u16,
    pub cols: u16,
    pub resume: PanelResume,
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    /// Scratch editor buffer content (persisted for file-less editors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_content: Option<String>,
}

impl PanelState {
    #[must_use]
    pub fn from_config(
        workspace_index: usize,
        workspace_name: &str,
        panel_index: usize,
        workspace: &WorkspaceConfig,
        workspace_position: [f32; 2],
        panel: &TerminalConfig,
    ) -> Self {
        let position = panel
            .position
            .map(|relative| [workspace_position[0] + relative[0], workspace_position[1] + relative[1]]);
        let cwd = normalize_cwd(panel.cwd.as_deref()).or_else(|| normalize_cwd(workspace.cwd.as_deref()));
        let command = panel.command.clone();
        let args = panel.args.clone();
        let ssh_connection = panel.ssh_connection.clone();

        Self {
            local_id: new_local_id(),
            name: panel.name.clone(),
            kind: panel.kind,
            command: command.clone(),
            args: args.clone(),
            cwd: cwd.clone(),
            ssh_connection: ssh_connection.clone(),
            rows: panel.rows,
            cols: panel.cols,
            resume: panel.resume.clone(),
            position,
            size: panel.size,
            session_binding: None,
            template: Some(PanelTemplateRef {
                workspace_index,
                workspace_name: workspace_name.to_string(),
                panel_index,
                kind: panel.kind,
                command,
                args,
                cwd,
                ssh_connection,
            }),
            editor_content: None,
        }
    }

    #[must_use]
    pub fn to_panel_options(&self) -> PanelOptions {
        PanelOptions {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            command: self.command.clone(),
            args: self.args.clone(),
            cwd: self.cwd.as_deref().map(Config::expand_tilde),
            ssh_connection: self.ssh_connection.clone(),
            rows: self.rows,
            cols: self.cols,
            kind: self.kind,
            resume: self.resume.clone(),
            position: self.position,
            size: self.size,
            local_id: Some(self.local_id.clone()),
            session_binding: self.session_binding.clone(),
            template: self.template.clone(),
            transcript_root: None,
        }
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            local_id: String::new(),
            name: String::new(),
            kind: PanelKind::default(),
            command: None,
            args: Vec::new(),
            cwd: None,
            ssh_connection: None,
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            resume: PanelResume::default(),
            position: None,
            size: None,
            session_binding: None,
            template: None,
            editor_content: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WorkspaceTemplateRef {
    pub workspace_index: usize,
    pub workspace_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PanelTemplateRef {
    pub workspace_index: usize,
    pub workspace_name: String,
    pub panel_index: usize,
    pub kind: PanelKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AgentSessionBinding {
    pub kind: PanelKind,
    pub session_id: String,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub updated_at: Option<i64>,
}

impl AgentSessionBinding {
    #[must_use]
    pub fn new(
        kind: PanelKind,
        session_id: String,
        cwd: Option<String>,
        label: Option<String>,
        updated_at: Option<i64>,
    ) -> Self {
        Self {
            kind,
            session_id,
            cwd,
            label,
            updated_at,
        }
    }
}

#[must_use]
pub fn new_local_id() -> String {
    Uuid::new_v4().to_string()
}

fn normalize_cwd(cwd: Option<&str>) -> Option<String> {
    cwd.map(Config::expand_tilde).map(|path| path.display().to_string())
}

fn empty_to_none(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_board_preserves_window_view_focus_and_bindings() {
        let mut board = Board::new();
        let alpha = board.create_workspace_at("alpha", [120.0, 64.0]);
        let beta = board.create_workspace_at("beta", [860.0, 64.0]);
        let panel_id = board
            .create_panel(
                PanelOptions {
                    name: Some("agent shell".to_string()),
                    position: Some([180.0, 120.0]),
                    size: Some([640.0, 420.0]),
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Shell,
                        "session-42".to_string(),
                        Some("/repo".to_string()),
                        Some("Agent shell".to_string()),
                        Some(17),
                    )),
                    ..PanelOptions::default()
                },
                beta,
            )
            .expect("panel should spawn");
        board.focus_workspace(alpha);
        board.focus(panel_id);

        let window = WindowConfig {
            width: 1920.0,
            height: 1080.0,
            x: Some(32.0),
            y: Some(48.0),
        };
        let state = RuntimeState::from_board(&board, window.clone(), CanvasViewState::new([24.0, -18.0], 1.6));

        let saved_window = state.window.expect("window config");
        assert!((saved_window.width - window.width).abs() <= f32::EPSILON);
        assert!((saved_window.height - window.height).abs() <= f32::EPSILON);
        assert_eq!(saved_window.x, window.x);
        assert_eq!(saved_window.y, window.y);
        assert_eq!(state.canvas_view, Some(CanvasViewState::new([24.0, -18.0], 1.6)));
        assert_eq!(state.pan_offset, None);
        assert_eq!(
            state.active_workspace_local_id.as_deref(),
            Some(board.workspace(beta).expect("workspace").local_id.as_str())
        );
        assert_eq!(
            state.focused_panel_local_id.as_deref(),
            Some(board.panel(panel_id).expect("panel").local_id.as_str())
        );
        assert_eq!(state.workspaces.len(), 2);

        let saved_workspace = state
            .workspaces
            .iter()
            .find(|workspace| workspace.local_id == board.workspace(beta).expect("workspace").local_id)
            .expect("workspace state");
        let saved_panel = saved_workspace.panels.first().expect("panel state");
        assert_eq!(saved_workspace.position, Some([860.0, 64.0]));
        assert_eq!(saved_workspace.layout, None);
        assert_eq!(saved_panel.position, Some([180.0, 120.0]));
        assert_eq!(saved_panel.size, Some([640.0, 420.0]));
        assert_eq!(
            saved_panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-42")
        );
    }

    #[test]
    fn from_board_persists_workspace_layout_selection() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace_at("grid", [860.0, 64.0]);
        board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("first panel should spawn");
        board
            .create_panel(PanelOptions::default(), workspace_id)
            .expect("second panel should spawn");
        board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

        let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let saved_workspace = state
            .workspaces
            .iter()
            .find(|workspace| workspace.local_id == board.workspace(workspace_id).expect("workspace").local_id)
            .expect("workspace state");

        assert_eq!(saved_workspace.layout, Some(WorkspaceLayout::Grid));
    }

    #[test]
    fn load_maps_removed_layout_variants_to_manual_placement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtime.yaml");
        std::fs::write(
            &path,
            r"version: 2
workspaces:
  - local_id: ws-stack
    name: Stack Workspace
    layout: Stack
    panels: []
  - local_id: ws-cascade
    name: Cascade Workspace
    layout: cascade
    panels: []
",
        )
        .expect("write runtime state");

        let state = RuntimeState::load(&path)
            .expect("load runtime state")
            .expect("runtime state present");

        assert_eq!(state.workspaces.len(), 2);
        assert!(state.workspaces.iter().all(|workspace| workspace.layout.is_none()));
    }

    #[test]
    fn load_migrates_legacy_pan_offset_into_canvas_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtime.yaml");
        std::fs::write(
            &path,
            r"version: 1
pan_offset:
  - 48.0
  - -12.0
workspaces: []
",
        )
        .expect("write runtime state");

        let state = RuntimeState::load(&path)
            .expect("load runtime state")
            .expect("runtime state present");

        assert_eq!(
            state.canvas_view,
            Some(CanvasViewState::from_legacy_pan_offset([48.0, -12.0]))
        );
        assert_eq!(state.pan_offset, None);
        assert!(state.has_persisted_canvas_view());
    }

    #[test]
    fn canvas_view_defaults_when_runtime_state_has_no_persisted_view() {
        let state = RuntimeState::default();

        assert_eq!(state.canvas_view_or_default(), CanvasViewState::default());
        assert!(!state.has_persisted_canvas_view());
    }

    #[test]
    fn from_board_persists_detached_workspaces() {
        let mut board = Board::new();
        let alpha = board.create_workspace_at("alpha", [120.0, 64.0]);
        let beta = board.create_workspace_at("beta", [860.0, 64.0]);

        let alpha_local_id = board.workspace(alpha).expect("alpha workspace").local_id.clone();
        let beta_local_id = board.workspace(beta).expect("beta workspace").local_id.clone();

        let detached_workspaces = vec![DetachedWorkspaceState {
            workspace_local_id: beta_local_id.clone(),
            window: WindowConfig {
                width: 1440.0,
                height: 900.0,
                x: Some(2560.0),
                y: Some(80.0),
            },
        }];

        let state = RuntimeState::from_board_with_detached_workspaces(
            &board,
            WindowConfig::default(),
            CanvasViewState::new([18.0, -12.0], 1.25),
            detached_workspaces.clone(),
        );

        assert_eq!(state.detached_workspaces.len(), 1);
        assert_eq!(state.detached_workspaces[0].workspace_local_id, beta_local_id);
        assert!((state.detached_workspaces[0].window.width - 1440.0).abs() <= f32::EPSILON);
        assert!((state.detached_workspaces[0].window.height - 900.0).abs() <= f32::EPSILON);
        assert_eq!(state.detached_workspaces[0].window.x, Some(2560.0));
        assert_eq!(state.detached_workspaces[0].window.y, Some(80.0));

        assert_eq!(
            state.workspaces[0].local_id, alpha_local_id,
            "workspace ordering should remain unchanged when detached metadata is added"
        );
    }

    #[test]
    fn to_yaml_omits_empty_detached_workspaces() {
        let yaml = RuntimeState::default().to_yaml().expect("serialize runtime state");

        assert!(!yaml.contains("detached_workspaces"));
    }

    #[test]
    fn load_preserves_detached_workspaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtime.yaml");
        std::fs::write(
            &path,
            r"version: 2
canvas_view:
  pan_offset:
    - 24.0
    - -12.0
  zoom: 1.5
detached_workspaces:
  - workspace_local_id: ws-beta
    window:
      width: 1440.0
      height: 900.0
      x: 2560.0
      y: 80.0
workspaces:
  - local_id: ws-alpha
    name: Alpha
    panels: []
  - local_id: ws-beta
    name: Beta
    panels: []
",
        )
        .expect("write runtime state");

        let state = RuntimeState::load(&path)
            .expect("load runtime state")
            .expect("runtime state present");

        assert_eq!(state.detached_workspaces.len(), 1);
        assert_eq!(state.detached_workspaces[0].workspace_local_id, "ws-beta");
        assert!((state.detached_workspaces[0].window.width - 1440.0).abs() <= f32::EPSILON);
        assert!((state.detached_workspaces[0].window.height - 900.0).abs() <= f32::EPSILON);
        assert_eq!(state.detached_workspaces[0].window.x, Some(2560.0));
        assert_eq!(state.detached_workspaces[0].window.y, Some(80.0));
        assert_eq!(state.canvas_view, Some(CanvasViewState::new([24.0, -12.0], 1.5)));
    }
}
