mod agent_sessions;
mod binding_bootstrap;
mod claude_live_sessions;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

pub use agent_sessions::{AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionRecord};
pub use claude_live_sessions::{claude_session_transcript_exists, live_claude_session_ids};

const RUNTIME_STATE_VERSION: u32 = 2;
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const MAX_CLAUDE_SESSION_FILES: usize = 64;
const MAX_PI_SESSION_FILES: usize = 128;
const CLAUDE_SESSION_HEAD_LINE_LIMIT: usize = 48;
const CLAUDE_SESSION_TAIL_LINE_LIMIT: usize = 24;
const CLAUDE_SESSION_TAIL_BYTES: u64 = 32 * 1024;
const PI_SESSION_HEAD_LINE_LIMIT: usize = 64;
const PI_SESSION_TAIL_LINE_LIMIT: usize = 64;
const PI_SESSION_TAIL_BYTES: u64 = 64 * 1024;

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
    /// Browser config injected by the app before restore. Core-only board
    /// snapshots store a default placeholder because a [`Board`] does not
    /// own the config that created it.
    #[serde(default)]
    pub browser: crate::browser::BrowserConfig,
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
            browser: config.browser.clone(),
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

        let mut reserved_panel_local_ids = self
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .filter(|panel| !panel.local_id.is_empty())
            .map(|panel| panel.local_id.clone())
            .collect::<HashSet<_>>();
        let mut seen_panel_local_ids = HashSet::new();

        for workspace in &mut self.workspaces {
            if workspace.local_id.is_empty() {
                workspace.local_id = new_local_id();
            }
            for panel in &mut workspace.panels {
                if panel.local_id.is_empty() {
                    panel.local_id = reserve_new_local_id(&mut reserved_panel_local_ids);
                    seen_panel_local_ids.insert(panel.local_id.clone());
                } else if !seen_panel_local_ids.insert(panel.local_id.clone()) {
                    // Focus restoration resolves the first matching panel.
                    // Keep that identity stable and repair later duplicates.
                    panel.local_id = reserve_new_local_id(&mut reserved_panel_local_ids);
                    seen_panel_local_ids.insert(panel.local_id.clone());
                }
            }
        }
    }

    /// Give browser panels fresh process-artifact identities when a persisted
    /// session is copied. Browser profiles and live manifests are keyed by
    /// panel local id, while duplicated sessions can run alongside their
    /// source; retaining those ids would make both Chrome drivers share and
    /// remove each other's files.
    pub(crate) fn regenerate_browser_local_ids(&mut self) {
        for workspace in &mut self.workspaces {
            for panel in &mut workspace.panels {
                if panel.kind != PanelKind::Browser {
                    continue;
                }
                let old_local_id = std::mem::replace(&mut panel.local_id, new_local_id());
                if self.focused_panel_local_id.as_deref() == Some(old_local_id.as_str()) {
                    self.focused_panel_local_id.clone_from(&Some(panel.local_id.clone()));
                }
            }
        }
    }

    pub fn migrate_canvas_view(&mut self) {
        self.canvas_view = Some(self.canvas_view_or_default());
        self.pan_offset = None;
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
                    .map(|panel| {
                        let terminal = panel.terminal();
                        let editor = panel.editor();
                        let browser = panel.browser();

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
                            browser_profile: browser.map(|browser| BrowserProfileState {
                                root: browser.profile_root_for_persistence().map(Path::to_path_buf),
                                backend: Some(browser.backend()),
                            }),
                            // `Some("")` is meaningful: Chrome committed its
                            // blank startup target, so restore must not fall
                            // back to the requested/configured URL.
                            browser_url: browser
                                .and_then(|browser| browser.committed_url_for_persistence().map(str::to_string)),
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
            // Save-path placeholder: the app refreshes this field with the
            // current config before any restore uses it.
            browser: crate::browser::BrowserConfig::default(),
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
            browser: crate::browser::BrowserConfig::default(),
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
        "grid" => Ok(Some(WorkspaceLayout::default())),
        "stack" | "cascade" => Ok(None),
        _ => Err(serde::de::Error::unknown_variant(
            &value,
            &["Rows", "Columns", "Grid", "Stack", "Cascade"],
        )),
    }
}

impl WorkspaceState {
    fn layout_from_config(workspace: &WorkspaceConfig) -> Option<WorkspaceLayout> {
        if workspace.terminals.iter().any(|panel| panel.position.is_some()) {
            None
        } else {
            Some(WorkspaceLayout::default())
        }
    }

    #[must_use]
    pub fn from_config(workspace_index: usize, workspace: &WorkspaceConfig, resolved_position: [f32; 2]) -> Self {
        let workspace_cwd = normalize_cwd(workspace.cwd.as_deref());
        let layout = Self::layout_from_config(workspace);
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
            layout,
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
    /// Profile root used when this Browser panel was launched. The outer
    /// option distinguishes a legacy snapshot from a panel launched with the
    /// default root, represented by `Some(BrowserProfileState { root: None })`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_profile: Option<BrowserProfileState>,
    /// Current URL of browser panels, restored as the navigation target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_url: Option<String>,
}

impl PanelState {
    /// The persisted session id, whether it came from a captured binding or
    /// an explicit `resume: session` setting.
    #[must_use]
    pub fn stored_session_id(&self) -> Option<&str> {
        self.session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .or(match &self.resume {
                PanelResume::Session { session_id } => Some(session_id.as_str()),
                PanelResume::Fresh | PanelResume::Last => None,
            })
    }

    fn ensure_session_binding(&mut self, session_id: &str) -> bool {
        let mut changed = false;
        self.session_binding.get_or_insert_with(|| {
            changed = true;
            AgentSessionBinding::new(
                self.kind,
                session_id.to_string(),
                self.cwd.clone(),
                Some(self.name.clone()),
                None,
            )
        });
        changed
    }

    fn replace_session_binding(&mut self, binding: AgentSessionBinding) -> bool {
        let resume_changed = if matches!(self.resume, PanelResume::Session { .. }) {
            let resume = PanelResume::Session {
                session_id: binding.session_id.clone(),
            };
            let changed = self.resume != resume;
            self.resume = resume;
            changed
        } else {
            false
        };
        let changed = resume_changed || self.session_binding.as_ref() != Some(&binding);
        self.session_binding = Some(binding);
        changed
    }

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
            browser_profile: None,
            browser_url: None,
        }
    }

    pub(crate) fn browser_config_for_restore(
        &self,
        fallback: &crate::browser::BrowserConfig,
    ) -> crate::browser::BrowserConfig {
        let mut config = fallback.clone();
        if let Some(profile) = &self.browser_profile {
            config.profile_root.clone_from(&profile.root);
            if let Some(backend) = profile.backend {
                config.backend = backend;
            }
        }
        config
    }

    #[must_use]
    pub fn to_panel_options(&self, browser_config: &crate::browser::BrowserConfig) -> PanelOptions {
        let command = if self.kind == PanelKind::Browser {
            self.browser_url.clone().or_else(|| self.command.clone())
        } else {
            self.command.clone()
        };
        PanelOptions {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            command,
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
            browser_config: (self.kind == PanelKind::Browser).then(|| self.browser_config_for_restore(browser_config)),
            transcript_root: None,
            restore_as_disconnected_snapshot: false,
            is_restore: true,
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
            browser_profile: None,
            browser_url: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserProfileState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::browser::BackendKind>,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AgentSessionKey {
    kind: PanelKind,
    session_id: String,
}

impl AgentSessionKey {
    #[must_use]
    pub fn new(kind: PanelKind, session_id: impl Into<String>) -> Self {
        Self {
            kind,
            session_id: session_id.into(),
        }
    }
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

fn reserve_new_local_id(reserved: &mut HashSet<String>) -> String {
    loop {
        let local_id = new_local_id();
        if reserved.insert(local_id.clone()) {
            return local_id;
        }
    }
}

fn normalize_cwd(cwd: Option<&str>) -> Option<String> {
    cwd.map(Config::expand_tilde).map(|path| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use crate::config::{TerminalConfig, WorkspaceConfig};

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
                    kind: PanelKind::Codex,
                    resume: PanelResume::Session {
                        session_id: "session-42".to_string(),
                    },
                    position: Some([180.0, 120.0]),
                    size: Some([640.0, 420.0]),
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-42".to_string(),
                        Some("/repo".to_string()),
                        Some("Codex session".to_string()),
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
    fn from_board_records_each_browser_launch_profile_root() {
        let temp = tempfile::tempdir().expect("temporary profile root");
        let launched_root = temp.path().join("launched-profiles");
        let mut board = Board::new();
        let workspace = board.create_workspace("browser");
        board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Browser,
                    local_id: Some("saved-browser".to_string()),
                    browser_config: Some(crate::browser::BrowserConfig {
                        command: Some(temp.path().join("missing-chrome").display().to_string()),
                        profile_root: Some(launched_root.clone()),
                        ..crate::browser::BrowserConfig::default()
                    }),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("Browser panel state should start");

        let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let saved = &state.workspaces[0].panels[0];

        assert_eq!(
            saved.browser_profile.as_ref().and_then(|profile| profile.root.as_ref()),
            Some(&launched_root)
        );
        board.shutdown_terminal_panels();
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
    fn workspace_state_from_config_defaults_layout_to_grid() {
        let workspace = WorkspaceConfig {
            name: "Alpha".to_string(),
            color: None,
            cwd: None,
            position: None,
            terminals: vec![TerminalConfig {
                name: "Shell".to_string(),
                ..TerminalConfig::default()
            }],
        };

        let state = WorkspaceState::from_config(0, &workspace, [120.0, 64.0]);

        assert_eq!(state.layout, Some(WorkspaceLayout::Grid));
    }

    #[test]
    fn empty_committed_browser_url_overrides_the_requested_command() {
        let panel = PanelState {
            kind: PanelKind::Browser,
            command: Some("https://requested.example".to_string()),
            browser_url: Some(String::new()),
            ..PanelState::default()
        };

        let options = panel.to_panel_options(&crate::browser::BrowserConfig::default());

        assert_eq!(options.command.as_deref(), Some(""));
    }

    #[test]
    fn persisted_browser_profile_root_survives_other_config_changes() {
        let panel = PanelState {
            kind: PanelKind::Browser,
            browser_profile: Some(BrowserProfileState {
                root: Some(PathBuf::from("/profiles/used-at-launch")),
                backend: None,
            }),
            ..PanelState::default()
        };
        let current_config = crate::browser::BrowserConfig {
            quality: 73,
            profile_root: Some(PathBuf::from("/profiles/new-config")),
            ..crate::browser::BrowserConfig::default()
        };

        let restored = panel
            .to_panel_options(&current_config)
            .browser_config
            .expect("Browser restore config");

        assert_eq!(restored.profile_root, Some(PathBuf::from("/profiles/used-at-launch")));
        assert_eq!(restored.quality, 73);
    }

    #[test]
    fn persisted_default_browser_profile_root_overrides_a_later_custom_root() {
        let panel = PanelState {
            kind: PanelKind::Browser,
            browser_profile: Some(BrowserProfileState::default()),
            ..PanelState::default()
        };
        let current_config = crate::browser::BrowserConfig {
            profile_root: Some(PathBuf::from("/profiles/new-config")),
            ..crate::browser::BrowserConfig::default()
        };

        let restored = panel
            .to_panel_options(&current_config)
            .browser_config
            .expect("Browser restore config");

        assert!(restored.profile_root.is_none());
    }

    #[test]
    fn persisted_default_browser_profile_root_survives_yaml_roundtrip() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                panels: vec![PanelState {
                    kind: PanelKind::Browser,
                    browser_profile: Some(BrowserProfileState::default()),
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        let yaml = state.to_yaml().expect("serialize runtime state");
        let restored: RuntimeState = serde_yaml::from_str(&yaml).expect("deserialize runtime state");

        assert!(restored.workspaces[0].panels[0].browser_profile.is_some());
        assert!(
            restored.workspaces[0].panels[0]
                .browser_profile
                .as_ref()
                .is_some_and(|profile| profile.root.is_none())
        );
    }

    #[test]
    fn ensure_local_ids_repairs_duplicates_without_moving_focus() {
        let mut state = RuntimeState {
            focused_panel_local_id: Some("shared-browser".to_string()),
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                panels: vec![
                    PanelState {
                        local_id: "shared-browser".to_string(),
                        kind: PanelKind::Browser,
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "shared-browser".to_string(),
                        kind: PanelKind::Browser,
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "shared-browser".to_string(),
                        ..PanelState::default()
                    },
                ],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        state.ensure_local_ids();

        let local_ids = state.workspaces[0]
            .panels
            .iter()
            .map(|panel| panel.local_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(local_ids[0], "shared-browser");
        assert_ne!(local_ids[1], "shared-browser");
        assert_ne!(local_ids[2], "shared-browser");
        assert_eq!(local_ids.iter().copied().collect::<HashSet<_>>().len(), 3);
        assert_eq!(state.focused_panel_local_id.as_deref(), Some("shared-browser"));
    }

    #[test]
    fn workspace_state_from_config_uses_manual_layout_when_any_panel_has_explicit_position() {
        let workspace = WorkspaceConfig {
            name: "Alpha".to_string(),
            color: None,
            cwd: None,
            position: None,
            terminals: vec![TerminalConfig {
                name: "Shell".to_string(),
                position: Some([120.0, 80.0]),
                ..TerminalConfig::default()
            }],
        };

        let state = WorkspaceState::from_config(0, &workspace, [120.0, 64.0]);

        assert_eq!(state.layout, None);
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
    fn pi_panel_state_round_trips_through_runtime_yaml() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Pi".to_string(),
                    kind: PanelKind::Pi,
                    resume: PanelResume::Session {
                        session_id: "pi-session-123".to_string(),
                    },
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Pi,
                        "pi-session-123".to_string(),
                        Some("/repo".to_string()),
                        Some("Fix the build".to_string()),
                        Some(42),
                    )),
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        let yaml = state.to_yaml().expect("serialize runtime state");
        assert!(yaml.contains("kind: pi"));

        let reloaded: RuntimeState = serde_yaml::from_str(&yaml).expect("deserialize runtime state");
        let panel = &reloaded.workspaces[0].panels[0];
        assert_eq!(panel.kind, PanelKind::Pi);
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("pi-session-123")
        );
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
