use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::{Config, TerminalConfig, WindowConfig, WorkspaceConfig};
use crate::error::{Error, Result};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelKind, PanelOptions, PanelResume};

const RUNTIME_STATE_VERSION: u32 = 1;
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const TILE_GAP: f32 = 20.0;
const WS_INNER_PAD: f32 = 20.0;
const WORKSPACE_GAP: f32 = 80.0;
const MAX_CLAUDE_SESSION_FILES: usize = 64;
const CLAUDE_SESSION_HEAD_LINE_LIMIT: usize = 48;
const CLAUDE_SESSION_TAIL_LINE_LIMIT: usize = 24;
const CLAUDE_SESSION_TAIL_BYTES: u64 = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RuntimeState {
    pub version: u32,
    pub window: Option<WindowConfig>,
    pub pan_offset: Option<[f32; 2]>,
    pub active_workspace_local_id: Option<String>,
    pub focused_panel_local_id: Option<String>,
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
            pan_offset: None,
            active_workspace_local_id: None,
            focused_panel_local_id: None,
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
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            version: RUNTIME_STATE_VERSION,
            window: None,
            pan_offset: None,
            active_workspace_local_id: None,
            focused_panel_local_id: None,
            workspaces: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspaceState {
    pub local_id: String,
    pub name: String,
    pub cwd: Option<String>,
    pub position: Option<[f32; 2]>,
    pub template: Option<WorkspaceTemplateRef>,
    pub panels: Vec<PanelState>,
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
    pub rows: u16,
    pub cols: u16,
    pub resume: PanelResume,
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
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

        Self {
            local_id: new_local_id(),
            name: panel.name.clone(),
            kind: panel.kind,
            command: command.clone(),
            args: args.clone(),
            cwd: cwd.clone(),
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
            }),
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
            rows: self.rows,
            cols: self.cols,
            kind: self.kind,
            resume: self.resume.clone(),
            position: self.position,
            size: self.size,
            local_id: Some(self.local_id.clone()),
            session_binding: self.session_binding.clone(),
            template: self.template.clone(),
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
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            resume: PanelResume::default(),
            position: None,
            size: None,
            session_binding: None,
            template: None,
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

#[derive(Clone, Debug, Default)]
pub struct AgentSessionCatalog {
    sessions: Vec<AgentSessionRecord>,
}

impl AgentSessionCatalog {
    /// Load recent Claude and Codex sessions from their local stores.
    ///
    /// # Errors
    ///
    /// Returns an error if one of the underlying local session stores cannot be opened.
    pub fn load() -> Result<Self> {
        let mut sessions = load_claude_sessions()?;
        sessions.extend(load_codex_sessions()?);
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(Self { sessions })
    }

    #[must_use]
    pub fn recent_for(&self, kind: PanelKind, cwd: Option<&str>) -> Vec<AgentSessionRecord> {
        let normalized_cwd = normalize_cwd(cwd);
        self.sessions
            .iter()
            .filter(|session| {
                session.kind == kind
                    && match (&normalized_cwd, &session.cwd) {
                        (Some(expected), Some(actual)) => expected == actual,
                        (None, _) => true,
                        _ => false,
                    }
            })
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct AgentSessionRecord {
    pub kind: PanelKind,
    pub session_id: String,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub updated_at: i64,
}

impl AgentSessionRecord {
    #[must_use]
    pub fn into_binding(self) -> AgentSessionBinding {
        AgentSessionBinding::new(self.kind, self.session_id, self.cwd, self.label, Some(self.updated_at))
    }
}

fn load_claude_sessions() -> Result<Vec<AgentSessionRecord>> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let projects_dir = home.join(".claude/projects");
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut session_paths = Vec::new();
    collect_claude_project_files(&projects_dir, &mut session_paths)?;
    session_paths.sort_by(|left, right| right.1.cmp(&left.1));
    session_paths.truncate(MAX_CLAUDE_SESSION_FILES);

    let mut sessions_by_id: HashMap<String, AgentSessionRecord> = HashMap::new();
    for (path, updated_at) in session_paths {
        match load_claude_project_session_summary(&path, updated_at) {
            Ok(Some(session)) => match sessions_by_id.get_mut(&session.session_id) {
                Some(existing) if session.updated_at > existing.updated_at => *existing = session,
                Some(_) => {}
                None => {
                    sessions_by_id.insert(session.session_id.clone(), session);
                }
            },
            Ok(None) => {}
            Err(error) => {
                tracing::warn!("failed loading Claude session {}: {error}", path.display());
            }
        }
    }

    let mut sessions: Vec<_> = sessions_by_id.into_values().collect();
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(sessions)
}

fn collect_claude_project_files(dir: &Path, files: &mut Vec<(PathBuf, i64)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_claude_project_files(&path, files)?;
        } else if path.extension().and_then(std::ffi::OsStr::to_str) == Some("jsonl") {
            let updated_at = file_updated_at_millis(&path)?;
            files.push((path, updated_at));
        }
    }
    Ok(())
}

fn load_claude_project_session_summary(path: &Path, updated_at: i64) -> Result<Option<AgentSessionRecord>> {
    let session_id = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::State(format!("invalid Claude session path {}", path.display())))?;
    let mut file = std::fs::File::open(path)?;
    let mut summary = ClaudeSessionSummary::default();
    scan_claude_session_reader(
        BufReader::new(file.try_clone()?),
        Some(CLAUDE_SESSION_HEAD_LINE_LIMIT),
        &mut summary,
    );
    if summary.last_prompt.is_none() {
        scan_claude_session_tail(&mut file, &mut summary)?;
    }
    Ok(summary.into_record(&session_id, updated_at))
}

#[cfg(test)]
fn parse_claude_project_session<R: BufRead>(
    reader: R,
    fallback_session_id: &str,
    fallback_updated_at: i64,
) -> Option<AgentSessionRecord> {
    let mut summary = ClaudeSessionSummary::default();
    scan_claude_session_reader(reader, None, &mut summary);
    summary.into_record(fallback_session_id, fallback_updated_at)
}

#[derive(Default)]
struct ClaudeSessionSummary {
    session_id: Option<String>,
    cwd: Option<String>,
    slug: Option<String>,
    last_prompt: Option<String>,
}

impl ClaudeSessionSummary {
    fn apply_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };

        if let Some(found_session_id) = value.get("sessionId").and_then(Value::as_str)
            && !found_session_id.is_empty()
        {
            self.session_id = Some(found_session_id.to_string());
        }

        if self.cwd.is_none()
            && let Some(found_cwd) = value.get("cwd").and_then(Value::as_str)
        {
            self.cwd = normalize_cwd(Some(found_cwd));
        }

        if self.slug.is_none()
            && let Some(found_slug) = value.get("slug").and_then(Value::as_str)
            && !found_slug.is_empty()
        {
            self.slug = Some(found_slug.to_string());
        }

        if let Some("last-prompt") = value.get("type").and_then(Value::as_str)
            && let Some(found_prompt) = value.get("lastPrompt").and_then(Value::as_str)
            && !found_prompt.is_empty()
        {
            self.last_prompt = Some(truncate_session_label(found_prompt));
        }
    }

    fn into_record(self, fallback_session_id: &str, fallback_updated_at: i64) -> Option<AgentSessionRecord> {
        let session_id = self
            .session_id
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_session_id.to_string());

        if session_id.is_empty() {
            return None;
        }

        Some(AgentSessionRecord {
            kind: PanelKind::Claude,
            session_id,
            cwd: self.cwd,
            label: self.last_prompt.or(self.slug).or(Some("Claude session".to_string())),
            updated_at: fallback_updated_at,
        })
    }
}

fn scan_claude_session_reader<R: BufRead>(mut reader: R, limit: Option<usize>, summary: &mut ClaudeSessionSummary) {
    let mut buffer = Vec::new();
    let mut index = 0usize;
    loop {
        if limit.is_some_and(|line_limit| index >= line_limit) {
            break;
        }
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buffer);
                summary.apply_line(line.trim_end_matches(['\r', '\n']));
                index += 1;
            }
        }
    }
}

fn scan_claude_session_tail(file: &mut std::fs::File, summary: &mut ClaudeSessionSummary) -> Result<()> {
    let file_len = file.metadata()?.len();
    let start = file_len.saturating_sub(CLAUDE_SESSION_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let tail_start = lines.len().saturating_sub(CLAUDE_SESSION_TAIL_LINE_LIMIT);
    for line in &lines[tail_start..] {
        summary.apply_line(line);
    }
    Ok(())
}

fn truncate_session_label(value: &str) -> String {
    const MAX_CHARS: usize = 64;

    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }

    let mut label: String = trimmed.chars().take(MAX_CHARS - 1).collect();
    label.push_str("...");
    label
}

fn file_updated_at_millis(path: &Path) -> Result<i64> {
    let modified = std::fs::metadata(path)?.modified()?;
    let elapsed = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::State(format!("failed to read mtime for {}: {error}", path.display())))?;
    i64::try_from(elapsed.as_millis()).map_err(|error| Error::State(error.to_string()))
}

fn load_codex_sessions() -> Result<Vec<AgentSessionRecord>> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let sqlite_path = home.join(".codex/state_5.sqlite");
    if !sqlite_path.exists() {
        return Ok(Vec::new());
    }

    let connection = Connection::open(sqlite_path).map_err(|error| Error::State(error.to_string()))?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, cwd, updated_at
             FROM threads
             WHERE archived = 0
             ORDER BY updated_at DESC",
        )
        .map_err(|error| Error::State(error.to_string()))?;

    let rows = statement
        .query_map([], |row| {
            Ok(AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: row.get(0)?,
                label: row.get::<_, String>(1).ok().filter(|title| !title.is_empty()),
                cwd: normalize_cwd(row.get::<_, String>(2).ok().as_deref()),
                updated_at: row.get::<_, i64>(3)?.saturating_mul(1000),
            })
        })
        .map_err(|error| Error::State(error.to_string()))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|error| Error::State(error.to_string()))?);
    }
    Ok(sessions)
}

#[must_use]
pub fn runtime_state_path_for_config(config_path: &Path) -> Option<PathBuf> {
    let base_dir = xdg_state_home()
        .or_else(default_home_state_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let stable_config_path = std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let key = stable_state_key(&stable_config_path.to_string_lossy());
    Some(base_dir.join("horizon/runtime").join(format!("{key}.yaml")))
}

#[must_use]
pub fn new_local_id() -> String {
    Uuid::new_v4().to_string()
}

#[must_use]
pub fn new_session_binding(kind: PanelKind, cwd: Option<String>, label: Option<String>) -> Option<AgentSessionBinding> {
    match kind {
        PanelKind::Claude => Some(AgentSessionBinding::new(
            kind,
            Uuid::new_v4().to_string(),
            cwd,
            label,
            None,
        )),
        PanelKind::Codex | PanelKind::Shell | PanelKind::Command => None,
    }
}

fn xdg_state_home() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
}

fn default_home_state_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".local/state"))
}

fn stable_state_key(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn normalize_cwd(cwd: Option<&str>) -> Option<String> {
    cwd.map(Config::expand_tilde).map(|path| path.display().to_string())
}

fn empty_to_none(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

fn workspace_slot_width() -> f32 {
    let columns = 3.0;
    let content = columns * DEFAULT_PANEL_SIZE[0] + (columns - 1.0) * TILE_GAP;
    content + 2.0 * WS_INNER_PAD + WORKSPACE_GAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bootstrap_assigns_distinct_sessions_per_group() {
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "termgalore".to_string(),
                cwd: Some("/repo".to_string()),
                position: None,
                template: None,
                panels: vec![
                    PanelState {
                        local_id: "a".to_string(),
                        name: "Claude A".to_string(),
                        kind: PanelKind::Claude,
                        cwd: Some("/repo".to_string()),
                        resume: PanelResume::Last,
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "b".to_string(),
                        name: "Claude B".to_string(),
                        kind: PanelKind::Claude,
                        cwd: Some("/repo".to_string()),
                        resume: PanelResume::Last,
                        ..PanelState::default()
                    },
                ],
            }],
            ..RuntimeState::default()
        };
        let catalog = AgentSessionCatalog {
            sessions: vec![
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-1".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 2,
                },
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-2".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 1,
                },
            ],
        };

        state.bootstrap_missing_agent_bindings(&catalog);

        let bindings: Vec<_> = state.workspaces[0]
            .panels
            .iter()
            .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone()))
            .collect();
        assert_eq!(bindings.len(), 2);
        assert_ne!(bindings[0], bindings[1]);
    }

    #[test]
    fn runtime_state_path_is_stable_for_config_path() {
        let path = PathBuf::from("/tmp/horizon/config.yaml");
        let first = runtime_state_path_for_config(&path).expect("state path");
        let second = runtime_state_path_for_config(&path).expect("state path");
        assert_eq!(first, second);
        assert!(first.to_string_lossy().ends_with(".yaml"));
    }

    #[test]
    fn parse_claude_project_session_uses_resumable_jsonl_session_id() {
        let jsonl = concat!(
            "{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n",
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n",
        );

        let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

        assert_eq!(session.kind, PanelKind::Claude);
        assert_eq!(session.session_id, "session-123");
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.label.as_deref(), Some("reply with ok only"));
        assert_eq!(session.updated_at, 42);
    }

    #[test]
    fn parse_claude_project_session_falls_back_to_filename_id() {
        let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n";

        let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

        assert_eq!(session.session_id, "fallback-id");
        assert_eq!(session.cwd, None);
        assert_eq!(session.label.as_deref(), Some("Claude session"));
        assert_eq!(session.updated_at, 7);
    }

    #[test]
    fn load_claude_project_session_summary_reads_head_and_tail_metadata() {
        let path = std::env::temp_dir().join(format!("horizon-claude-session-{}.jsonl", Uuid::new_v4()));
        let mut content = String::from(
            "{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n",
        );
        for _ in 0..80 {
            content.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n");
        }
        content.push_str(
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n",
        );
        std::fs::write(&path, content).expect("write temp session file");

        let session = load_claude_project_session_summary(&path, 9)
            .expect("load")
            .expect("session");
        std::fs::remove_file(&path).ok();

        assert_eq!(session.kind, PanelKind::Claude);
        assert_eq!(session.session_id, "session-123");
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.label.as_deref(), Some("reply with ok only"));
        assert_eq!(session.updated_at, 9);
    }
}
