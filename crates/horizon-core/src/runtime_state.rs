mod agent_sessions;
mod binding_bootstrap;
mod claude_live_sessions;
mod models;
mod versioning;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::board::Board;
use crate::config::{Config, WindowConfig};
use crate::error::{Error, Result};
use crate::layout::workspace_slot_width;
use crate::panel::PanelKind;
use crate::terminal::Terminal;
use crate::view::CanvasViewState;

pub use agent_sessions::{AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionRecord};
pub use claude_live_sessions::{claude_session_transcript_exists, live_claude_session_ids};
pub use models::{
    AgentSessionBinding, AgentSessionKey, BrowserProfileState, DetachedWorkspaceState, PanelState, PanelTemplateRef,
    WorkspaceState, WorkspaceTemplateRef,
};

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
    #[serde(with = "versioning")]
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
    /// Returns an error if the state file exists but cannot be read or parsed,
    /// or its version is newer than this binary supports. Loading never rewrites
    /// the source file, including when migration fails.
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
    /// Returns an error if serialization fails or the state version is unsupported.
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
                            name_is_custom: Some(panel.name_is_custom()),
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
                                hidden: !panel.visible,
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

/// Lexically normalize a configured cwd so equivalent spellings match in
/// history lookups: `~` expands, and trailing or duplicate separators plus
/// interior `.` components fold. Parent components and symlinks remain unresolved.
fn normalize_cwd(cwd: Option<&str>) -> Option<String> {
    cwd.map(Config::expand_tilde)
        .map(|path| path.components().collect::<PathBuf>())
        .map(|path| path.display().to_string())
}

#[cfg(test)]
mod tests;
