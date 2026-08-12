use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::local_store::open_read_only_sqlite;
use crate::opencode_paths::opencode_db_path;

use super::{AgentSessionBinding, PanelKind, RuntimeState, normalize_cwd};

mod codex;

#[derive(Clone, Debug, Default)]
pub struct AgentSessionCatalog {
    sessions: Vec<AgentSessionRecord>,
}

#[derive(Clone)]
pub(super) enum ExactSessionResolution {
    Verified,
    Rebind(AgentSessionBinding),
    Unavailable,
}

pub struct AgentSessionBootstrapCatalog {
    catalog: AgentSessionCatalog,
    exact_resolutions: HashMap<(PanelKind, String), ExactSessionResolution>,
}

impl AgentSessionCatalog {
    /// Load recent Claude, Codex, `OpenCode`, and Pi sessions from their local stores.
    ///
    /// # Errors
    ///
    /// Returns an error if one of the underlying local session stores cannot be opened.
    pub fn load() -> Result<Self> {
        Self::load_strict(
            load_claude_sessions,
            || codex::load_sessions(&HashSet::new(), true),
            load_opencode_sessions,
            load_pi_sessions,
        )
    }

    /// Load only the provider catalogs needed to repair or assign bindings at startup.
    pub fn load_for_runtime_state(runtime_state: &RuntimeState) -> AgentSessionBootstrapCatalog {
        let exact_binding_ids: HashMap<PanelKind, HashSet<String>> = runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .filter(|panel| panel.kind.requires_exact_session_validation())
            .filter_map(|panel| panel.stored_session_id().map(|id| (panel.kind, id.to_string())))
            .fold(HashMap::new(), |mut grouped, (kind, session_id)| {
                grouped.entry(kind).or_insert_with(HashSet::new).insert(session_id);
                grouped
            });
        let needs_discovery = runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .any(|panel| {
                panel.kind.supports_session_binding()
                    && panel.session_binding.is_none()
                    && matches!(panel.resume, super::PanelResume::Last)
            });

        let mut sessions = Vec::new();
        if needs_discovery {
            extend_best_effort(&mut sessions, "Claude", load_claude_sessions());
            extend_best_effort(&mut sessions, "OpenCode", load_opencode_sessions());
            extend_best_effort(&mut sessions, "Pi", load_pi_sessions());
        }
        let codex_binding_ids = exact_binding_ids.get(&PanelKind::Codex).cloned().unwrap_or_default();
        let codex = match codex::load_sessions(&codex_binding_ids, needs_discovery) {
            Ok(codex) => codex,
            Err(error) => {
                tracing::warn!("failed loading Codex sessions: {error}");
                codex::CodexSessions::unavailable(&codex_binding_ids)
            }
        };
        AgentSessionBootstrapCatalog::new(Self::from_provider_sessions(sessions, &codex), codex)
    }

    fn load_strict(
        claude: impl FnOnce() -> Result<Vec<AgentSessionRecord>>,
        codex: impl FnOnce() -> Result<codex::CodexSessions>,
        opencode: impl FnOnce() -> Result<Vec<AgentSessionRecord>>,
        pi: impl FnOnce() -> Result<Vec<AgentSessionRecord>>,
    ) -> Result<Self> {
        let mut sessions = claude()?;
        let codex = codex()?;
        sessions.extend(opencode()?);
        sessions.extend(pi()?);
        Ok(Self::from_provider_sessions(sessions, &codex))
    }

    fn from_provider_sessions(mut sessions: Vec<AgentSessionRecord>, codex: &codex::CodexSessions) -> Self {
        sessions.extend(codex.sessions.iter().cloned());
        sessions.retain(|session| session.interactive);
        sessions.sort_by_key(|session| Reverse(session.updated_at));
        Self { sessions }
    }

    #[must_use]
    pub fn recent_for(&self, kind: PanelKind, cwd: Option<&str>) -> Vec<AgentSessionRecord> {
        let normalized_cwd = normalize_cwd(cwd);
        self.sessions
            .iter()
            .filter(|session| {
                session.interactive
                    && session.kind == kind
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

impl AgentSessionBootstrapCatalog {
    fn new(catalog: AgentSessionCatalog, codex: codex::CodexSessions) -> Self {
        let mut exact_resolutions = HashMap::new();
        exact_resolutions.extend(
            codex
                .verified_binding_ids
                .into_iter()
                .map(|session_id| ((PanelKind::Codex, session_id), ExactSessionResolution::Verified)),
        );
        exact_resolutions.extend(codex.root_aliases.into_iter().map(|(session_id, root)| {
            (
                (PanelKind::Codex, session_id),
                ExactSessionResolution::Rebind(root.into_binding()),
            )
        }));
        exact_resolutions.extend(
            codex
                .unavailable_binding_ids
                .into_iter()
                .map(|session_id| ((PanelKind::Codex, session_id), ExactSessionResolution::Unavailable)),
        );
        Self {
            catalog,
            exact_resolutions,
        }
    }

    pub(super) fn exact_resolution(&self, kind: PanelKind, session_id: &str) -> ExactSessionResolution {
        self.exact_resolutions
            .get(&(kind, session_id.to_string()))
            .cloned()
            .unwrap_or(ExactSessionResolution::Unavailable)
    }

    pub(super) fn recent_for(&self, kind: PanelKind, cwd: Option<&str>) -> Vec<AgentSessionRecord> {
        self.catalog.recent_for(kind, cwd)
    }

    #[must_use]
    pub fn into_catalog(self) -> AgentSessionCatalog {
        self.catalog
    }
}

fn extend_best_effort(sessions: &mut Vec<AgentSessionRecord>, provider: &str, loaded: Result<Vec<AgentSessionRecord>>) {
    match loaded {
        Ok(loaded) => sessions.extend(loaded),
        Err(error) => tracing::warn!("failed loading {provider} sessions: {error}"),
    }
}

#[derive(Clone, Debug)]
pub struct AgentSessionRecord {
    pub kind: PanelKind,
    pub session_id: String,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub updated_at: i64,
    pub interactive: bool,
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
    session_paths.sort_by_key(|(_, updated_at)| Reverse(*updated_at));
    session_paths.truncate(super::MAX_CLAUDE_SESSION_FILES);

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
    sessions.sort_by_key(|session| Reverse(session.updated_at));
    Ok(sessions)
}

fn collect_claude_project_files(dir: &Path, files: &mut Vec<(PathBuf, i64)>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!("skipping unreadable Claude project dir {}: {error}", dir.display());
            return Ok(());
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            // Skip subagent session directories - they share the parent
            // session ID and would only dilute the file limit.
            if path.file_name().and_then(std::ffi::OsStr::to_str) == Some("subagents") {
                continue;
            }
            collect_claude_project_files(&path, files)?;
        } else if path.extension().and_then(std::ffi::OsStr::to_str) == Some("jsonl")
            && let Ok(updated_at) = file_updated_at_millis(&path)
        {
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
        Some(super::CLAUDE_SESSION_HEAD_LINE_LIMIT),
        &mut summary,
    );
    if summary.last_prompt.is_none() {
        scan_claude_session_tail(&mut file, &mut summary)?;
    }
    Ok(summary.into_record(&session_id, updated_at))
}

#[derive(Default)]
struct ClaudeSessionSummary {
    session_id: Option<String>,
    cwd: Option<String>,
    slug: Option<String>,
    last_prompt: Option<String>,
    parent_controlled: bool,
}

impl ClaudeSessionSummary {
    fn apply_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };

        self.parent_controlled |= value.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || value
                .get("parentUuid")
                .and_then(Value::as_str)
                .is_some_and(|parent| !parent.is_empty());

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
            interactive: !self.parent_controlled,
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
    let start = file_len.saturating_sub(super::CLAUDE_SESSION_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let tail_start = lines.len().saturating_sub(super::CLAUDE_SESSION_TAIL_LINE_LIMIT);
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

fn load_opencode_sessions() -> Result<Vec<AgentSessionRecord>> {
    let Some(sqlite_path) = opencode_db_path() else {
        return Ok(Vec::new());
    };
    if !sqlite_path.exists() {
        return Ok(Vec::new());
    }
    load_opencode_sessions_from_path(&sqlite_path)
}

fn load_opencode_sessions_from_path(sqlite_path: &Path) -> Result<Vec<AgentSessionRecord>> {
    let connection = open_read_only_sqlite(sqlite_path)?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, directory, time_updated
             FROM session
             WHERE time_archived IS NULL
               AND parent_id IS NULL
             ORDER BY time_updated DESC",
        )
        .map_err(|error| Error::State(error.to_string()))?;

    let rows = statement
        .query_map([], |row| {
            Ok(AgentSessionRecord {
                kind: PanelKind::OpenCode,
                session_id: row.get(0)?,
                label: row.get::<_, String>(1).ok().filter(|title| !title.is_empty()),
                cwd: normalize_cwd(row.get::<_, String>(2).ok().as_deref()),
                updated_at: row.get::<_, i64>(3)?,
                interactive: true,
            })
        })
        .map_err(|error| Error::State(error.to_string()))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|error| Error::State(error.to_string()))?);
    }
    Ok(sessions)
}

fn load_pi_sessions() -> Result<Vec<AgentSessionRecord>> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let sessions_dir = home.join(".pi/agent/sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    load_pi_sessions_from_dir(&sessions_dir)
}

fn load_pi_sessions_from_dir(sessions_dir: &Path) -> Result<Vec<AgentSessionRecord>> {
    let mut session_paths = Vec::new();
    collect_pi_session_files(sessions_dir, &mut session_paths)?;
    session_paths.sort_by_key(|(_, updated_at)| Reverse(*updated_at));
    session_paths.truncate(super::MAX_PI_SESSION_FILES);

    let mut sessions = Vec::new();
    for (path, updated_at) in session_paths {
        match load_pi_session_summary(&path, updated_at) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(error) => tracing::warn!("failed loading Pi session {}: {error}", path.display()),
        }
    }
    sessions.sort_by_key(|session| Reverse(session.updated_at));
    Ok(sessions)
}

fn collect_pi_session_files(dir: &Path, files: &mut Vec<(PathBuf, i64)>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!("skipping unreadable Pi session dir {}: {error}", dir.display());
            return Ok(());
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_pi_session_files(&path, files)?;
        } else if path.extension().and_then(std::ffi::OsStr::to_str) == Some("jsonl")
            && let Ok(updated_at) = file_updated_at_millis(&path)
        {
            files.push((path, updated_at));
        }
    }
    Ok(())
}

fn load_pi_session_summary(path: &Path, updated_at: i64) -> Result<Option<AgentSessionRecord>> {
    let fallback_session_id = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::State(format!("invalid Pi session path {}", path.display())))?;
    let mut file = std::fs::File::open(path)?;
    let mut summary = PiSessionSummary::default();
    scan_pi_session_reader(
        BufReader::new(file.try_clone()?),
        Some(super::PI_SESSION_HEAD_LINE_LIMIT),
        &mut summary,
    );
    scan_pi_session_tail(&mut file, &mut summary)?;
    Ok(summary.into_record(&fallback_session_id, updated_at))
}

#[derive(Default)]
struct PiSessionSummary {
    session_id: Option<String>,
    cwd: Option<String>,
    last_user_message: Option<String>,
    parent_controlled: bool,
}

impl PiSessionSummary {
    fn apply_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };

        self.parent_controlled |= string_field(&value, &["parent_id", "parentId", "parent_session_id"])
            .or_else(|| nested_string_field(&value, "session", &["parent_id", "parentId", "parent_session_id"]))
            .or_else(|| nested_string_field(&value, "metadata", &["parent_id", "parentId", "parent_session_id"]))
            .is_some_and(|parent| !parent.is_empty());

        if self.session_id.is_none()
            && let Some(found_session_id) = extract_pi_session_id(&value)
            && !found_session_id.is_empty()
        {
            self.session_id = Some(found_session_id.to_string());
        }

        if self.cwd.is_none()
            && let Some(found_cwd) = extract_pi_cwd(&value)
        {
            self.cwd = normalize_cwd(Some(found_cwd));
        }

        if let Some(user_message) = extract_pi_user_message(&value) {
            self.last_user_message = Some(truncate_session_label(&user_message));
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
            kind: PanelKind::Pi,
            session_id,
            cwd: self.cwd,
            label: self.last_user_message.or_else(|| Some("Pi session".to_string())),
            updated_at: fallback_updated_at,
            interactive: !self.parent_controlled,
        })
    }
}

fn scan_pi_session_reader<R: BufRead>(mut reader: R, limit: Option<usize>, summary: &mut PiSessionSummary) {
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

fn scan_pi_session_tail(file: &mut std::fs::File, summary: &mut PiSessionSummary) -> Result<()> {
    let file_len = file.metadata()?.len();
    let start = file_len.saturating_sub(super::PI_SESSION_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let tail_start = lines.len().saturating_sub(super::PI_SESSION_TAIL_LINE_LIMIT);
    for line in &lines[tail_start..] {
        summary.apply_line(line);
    }
    Ok(())
}

fn extract_pi_session_id(value: &Value) -> Option<&str> {
    string_field(value, &["session_id", "sessionId", "sessionID"])
        .or_else(|| nested_string_field(value, "session", &["id", "session_id", "sessionId"]))
        .or_else(|| {
            let record_kind = string_field(value, &["type", "event", "kind"])?;
            let normalized_kind = record_kind.to_ascii_lowercase();
            let is_session_record = normalized_kind.contains("session")
                || matches!(normalized_kind.as_str(), "agent_start" | "conversation_start");
            is_session_record.then(|| string_field(value, &["id"])).flatten()
        })
}

fn extract_pi_cwd(value: &Value) -> Option<&str> {
    string_field(value, &["cwd", "working_directory", "workingDirectory"])
        .or_else(|| nested_string_field(value, "session", &["cwd", "working_directory", "workingDirectory"]))
        .or_else(|| nested_string_field(value, "metadata", &["cwd", "working_directory", "workingDirectory"]))
        .or_else(|| nested_string_field(value, "context", &["cwd", "working_directory", "workingDirectory"]))
}

fn extract_pi_user_message(value: &Value) -> Option<String> {
    let root_role = string_field(value, &["role"]);
    let message_role = nested_string_field(value, "message", &["role"]);
    let record_kind = string_field(value, &["type", "event", "kind"]);
    let is_user = root_role
        .or(message_role)
        .is_some_and(|role| role.eq_ignore_ascii_case("user"))
        || record_kind.is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "user" | "user_message"));

    if !is_user {
        return None;
    }

    for key in ["text", "content", "prompt", "message"] {
        if let Some(text) = value.get(key).and_then(text_from_json_value) {
            return Some(text);
        }
    }
    None
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn nested_string_field<'a>(value: &'a Value, object_key: &str, keys: &[&str]) -> Option<&'a str> {
    value.get(object_key).and_then(|nested| string_field(nested, keys))
}

fn text_from_json_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty_text(text),
        Value::Array(values) => {
            let parts: Vec<_> = values.iter().filter_map(text_from_json_value).collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        Value::Object(_) => {
            for key in ["text", "content", "message", "value", "input"] {
                if let Some(text) = value.get(key).and_then(text_from_json_value) {
                    return Some(text);
                }
            }
            value.get("parts").and_then(text_from_json_value)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn non_empty_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Cursor;

    use rusqlite::Connection;
    use uuid::Uuid;

    use super::super::{AgentSessionBinding, PanelResume, PanelState, RuntimeState, WorkspaceState};
    use super::{
        AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionRecord, ClaudeSessionSummary,
        ExactSessionResolution, PanelKind, PiSessionSummary, codex::CodexSessions, load_claude_project_session_summary,
        load_opencode_sessions_from_path, load_pi_sessions_from_dir, scan_claude_session_reader,
        scan_pi_session_reader,
    };
    use crate::error::Error;

    #[test]
    fn strict_catalog_load_propagates_provider_errors() {
        let loaded = AgentSessionCatalog::load_strict(
            || Ok(Vec::new()),
            || Ok(CodexSessions::default()),
            || Err(Error::State("OpenCode store unavailable".to_string())),
            || Ok(Vec::new()),
        );

        assert!(matches!(loaded, Err(Error::State(message)) if message == "OpenCode store unavailable"));
    }

    fn parse_claude_project_session<R: std::io::BufRead>(
        reader: R,
        fallback_session_id: &str,
        fallback_updated_at: i64,
    ) -> Option<AgentSessionRecord> {
        let mut summary = ClaudeSessionSummary::default();
        scan_claude_session_reader(reader, None, &mut summary);
        summary.into_record(fallback_session_id, fallback_updated_at)
    }

    fn parse_pi_session<R: std::io::BufRead>(
        reader: R,
        fallback_session_id: &str,
        fallback_updated_at: i64,
    ) -> Option<AgentSessionRecord> {
        let mut summary = PiSessionSummary::default();
        scan_pi_session_reader(reader, None, &mut summary);
        summary.into_record(fallback_session_id, fallback_updated_at)
    }

    fn bootstrap_catalog(
        sessions: Vec<AgentSessionRecord>,
        exact_resolutions: impl IntoIterator<Item = ((PanelKind, String), ExactSessionResolution)>,
    ) -> AgentSessionBootstrapCatalog {
        AgentSessionBootstrapCatalog {
            catalog: AgentSessionCatalog { sessions },
            exact_resolutions: exact_resolutions.into_iter().collect(),
        }
    }

    #[test]
    fn bootstrap_assigns_distinct_sessions_per_group() {
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "termgalore".to_string(),
                cwd: Some("/repo".to_string()),
                position: None,
                template: None,
                layout: None,
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
        let catalog = bootstrap_catalog(
            vec![
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-1".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 2,
                    interactive: true,
                },
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-2".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 1,
                    interactive: true,
                },
            ],
            [],
        );

        state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

        let bindings: Vec<_> = state.workspaces[0]
            .panels
            .iter()
            .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone()))
            .collect();
        assert_eq!(bindings.len(), 2);
        assert_ne!(bindings[0], bindings[1]);
    }

    #[test]
    fn bootstrap_never_assigns_sessions_open_in_other_processes() {
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "termgalore".to_string(),
                cwd: Some("/repo".to_string()),
                position: None,
                template: None,
                layout: None,
                panels: vec![PanelState {
                    local_id: "a".to_string(),
                    name: "Claude A".to_string(),
                    kind: PanelKind::Claude,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Last,
                    ..PanelState::default()
                }],
            }],
            ..RuntimeState::default()
        };
        let catalog = bootstrap_catalog(
            vec![
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-live".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 2,
                    interactive: true,
                },
                AgentSessionRecord {
                    kind: PanelKind::Claude,
                    session_id: "session-free".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 1,
                    interactive: true,
                },
            ],
            [],
        );
        let busy = HashSet::from(["session-live".to_string()]);

        state.bootstrap_missing_agent_bindings(&catalog, &busy);

        let binding = state.workspaces[0].panels[0]
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.clone());
        assert_eq!(binding.as_deref(), Some("session-free"));
    }

    #[test]
    fn bootstrap_repairs_persisted_codex_child_bindings() {
        let root = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-root".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Root session".to_string()),
            updated_at: 42,
            interactive: true,
        };
        let fallback = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-fallback".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Fallback session".to_string()),
            updated_at: 41,
            interactive: true,
        };
        let catalog = bootstrap_catalog(
            vec![root.clone(), fallback],
            [(
                (PanelKind::Codex, "session-child".to_string()),
                ExactSessionResolution::Rebind(root.into_binding()),
            )],
        );
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Fresh,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-child".to_string(),
                        Some("/repo".to_string()),
                        None,
                        Some(50),
                    )),
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        let changed = state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

        assert!(changed);
        let panel = &state.workspaces[0].panels[0];
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-root")
        );
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .and_then(|binding| binding.label.as_deref()),
            Some("Root session")
        );
        assert!(matches!(
            &panel.resume,
            PanelResume::Session { session_id } if session_id == "session-root"
        ));
    }

    #[test]
    fn bootstrap_repairs_an_explicit_codex_child_resume() {
        let root = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-root".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Root session".to_string()),
            updated_at: 42,
            interactive: true,
        };
        let fallback = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-fallback".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Fallback session".to_string()),
            updated_at: 41,
            interactive: true,
        };
        let catalog = bootstrap_catalog(
            vec![root.clone(), fallback],
            [(
                (PanelKind::Codex, "session-child".to_string()),
                ExactSessionResolution::Rebind(root.into_binding()),
            )],
        );
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    session_binding: None,
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

        let panel = &state.workspaces[0].panels[0];
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-root")
        );
        assert!(matches!(
            &panel.resume,
            PanelResume::Session { session_id } if session_id == "session-root"
        ));
    }

    #[test]
    fn bootstrap_does_not_duplicate_a_root_resume() {
        let root = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-root".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Root session".to_string()),
            updated_at: 42,
            interactive: true,
        };
        let fallback = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-fallback".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Fallback session".to_string()),
            updated_at: 41,
            interactive: true,
        };
        let catalog = bootstrap_catalog(
            vec![root.clone(), fallback],
            [
                (
                    (PanelKind::Codex, "session-root".to_string()),
                    ExactSessionResolution::Verified,
                ),
                (
                    (PanelKind::Codex, "session-child-a".to_string()),
                    ExactSessionResolution::Rebind(root.clone().into_binding()),
                ),
                (
                    (PanelKind::Codex, "session-child-b".to_string()),
                    ExactSessionResolution::Rebind(root.into_binding()),
                ),
            ],
        );
        let binding = |session_id: &str| {
            AgentSessionBinding::new(
                PanelKind::Codex,
                session_id.to_string(),
                Some("/repo".to_string()),
                None,
                Some(50),
            )
        };
        let panel = |local_id: &str, session_id: &str| PanelState {
            local_id: local_id.to_string(),
            name: local_id.to_string(),
            kind: PanelKind::Codex,
            cwd: Some("/repo".to_string()),
            resume: PanelResume::Last,
            session_binding: Some(binding(session_id)),
            ..PanelState::default()
        };
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![
                    panel("root", "session-root"),
                    panel("child-a", "session-child-a"),
                    panel("child-b", "session-child-b"),
                ],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

        assert_eq!(
            state.workspaces[0].panels[0]
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-root")
        );
        let child_bindings: Vec<_> = state.workspaces[0].panels[1..]
            .iter()
            .filter_map(|panel| panel.session_binding.as_ref())
            .collect();
        assert_eq!(child_bindings.len(), 2);
        assert!(child_bindings.iter().all(|binding| !binding.resumable));
        assert_eq!(child_bindings[0].session_id, "session-child-a");
        assert_eq!(child_bindings[1].session_id, "session-child-b");
        for child in &state.workspaces[0].panels[1..] {
            assert!(matches!(child.resume, PanelResume::Last));
        }
    }

    #[test]
    fn bootstrap_retains_the_later_duplicate_direct_root_without_resuming_it() {
        let catalog = bootstrap_catalog(
            Vec::new(),
            [(
                (PanelKind::Codex, "session-root".to_string()),
                ExactSessionResolution::Verified,
            )],
        );
        let panel = |local_id: &str| PanelState {
            local_id: local_id.to_string(),
            name: local_id.to_string(),
            kind: PanelKind::Codex,
            resume: PanelResume::Last,
            session_binding: Some(AgentSessionBinding::new(
                PanelKind::Codex,
                "session-root".to_string(),
                Some("/repo".to_string()),
                None,
                None,
            )),
            ..PanelState::default()
        };
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                panels: vec![panel("first"), panel("second")],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

        let panels = &state.workspaces[0].panels;
        assert!(
            panels[0]
                .session_binding
                .as_ref()
                .is_some_and(|binding| binding.resumable)
        );
        assert!(
            panels[1]
                .session_binding
                .as_ref()
                .is_some_and(|binding| !binding.resumable)
        );
        assert_eq!(panels[1].stored_session_id(), Some("session-root"));
    }

    #[test]
    fn bootstrap_retains_an_unresolved_codex_child_binding() {
        let catalog = bootstrap_catalog(
            Vec::new(),
            [(
                (PanelKind::Codex, "session-child".to_string()),
                ExactSessionResolution::Unavailable,
            )],
        );
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    session_binding: None,
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

        let panel = &state.workspaces[0].panels[0];
        assert_eq!(panel.stored_session_id(), Some("session-child"));
        assert!(panel.session_binding.as_ref().is_some_and(|binding| !binding.resumable));
        assert!(matches!(panel.resume, PanelResume::Fresh));
    }

    #[test]
    fn bootstrap_preserves_last_for_an_unresolved_codex_child_binding() {
        let fallback = AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-fallback".to_string(),
            cwd: Some("/repo".to_string()),
            label: Some("Fallback session".to_string()),
            updated_at: 41,
            interactive: true,
        };
        let catalog = bootstrap_catalog(
            vec![fallback],
            [(
                (PanelKind::Codex, "session-child".to_string()),
                ExactSessionResolution::Unavailable,
            )],
        );
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Last,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-child".to_string(),
                        Some("/repo".to_string()),
                        None,
                        None,
                    )),
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

        let panel = &state.workspaces[0].panels[0];
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-child")
        );
        assert!(panel.session_binding.as_ref().is_some_and(|binding| !binding.resumable));
        assert!(matches!(panel.resume, PanelResume::Last));
    }

    #[test]
    fn explicit_recovery_retains_unverified_ids_without_resuming_them() {
        let binding = AgentSessionBinding::new(
            PanelKind::Codex,
            "session-child".to_string(),
            Some("/repo".to_string()),
            None,
            None,
        );
        let mut state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "horizon".to_string(),
                panels: vec![
                    PanelState {
                        local_id: "last".to_string(),
                        name: "Last".to_string(),
                        kind: PanelKind::Codex,
                        resume: PanelResume::Last,
                        session_binding: Some(binding),
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "explicit".to_string(),
                        name: "Explicit".to_string(),
                        kind: PanelKind::Codex,
                        resume: PanelResume::Session {
                            session_id: "session-missing".to_string(),
                        },
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "other".to_string(),
                        name: "Other".to_string(),
                        kind: PanelKind::OpenCode,
                        resume: PanelResume::Session {
                            session_id: "other-session".to_string(),
                        },
                        ..PanelState::default()
                    },
                ],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(state.retain_unverified_session_bindings());

        let panels = &state.workspaces[0].panels;
        assert_eq!(panels[0].stored_session_id(), Some("session-child"));
        assert!(
            panels[0]
                .session_binding
                .as_ref()
                .is_some_and(|binding| !binding.resumable)
        );
        assert!(matches!(panels[0].resume, PanelResume::Last));
        assert_eq!(panels[1].stored_session_id(), Some("session-missing"));
        assert!(
            panels[1]
                .session_binding
                .as_ref()
                .is_some_and(|binding| !binding.resumable)
        );
        assert!(matches!(panels[1].resume, PanelResume::Fresh));
        assert_eq!(panels[2].exact_session_id(), Some("other-session"));
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
    fn parse_claude_project_session_marks_sidechains_noninteractive() {
        let jsonl = "{\"type\":\"user\",\"sessionId\":\"child\",\"isSidechain\":true,\"parentUuid\":\"root\"}\n";

        let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

        assert!(!session.interactive);
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

    #[test]
    fn load_opencode_sessions_reads_root_sessions_from_sqlite() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let sqlite_path = temp_dir.path().join("opencode.db");
        let conn = Connection::open(&sqlite_path).expect("sqlite");
        conn.execute_batch(
            "\
CREATE TABLE session (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    directory TEXT NOT NULL,
    parent_id TEXT,
    time_updated INTEGER NOT NULL,
    time_archived INTEGER
);
INSERT INTO session (id, title, directory, parent_id, time_updated, time_archived) VALUES
    ('session-root', 'Fix auth flow', '/repo', NULL, 1000, NULL),
    ('session-child', 'Child', '/repo', 'session-root', 2000, NULL),
    ('session-archived', 'Archived', '/repo', NULL, 3000, 1),
    ('session-other', 'Other repo', '/other', NULL, 4000, NULL);
",
        )
        .expect("seed");

        let sessions = load_opencode_sessions_from_path(&sqlite_path).expect("opencode sessions");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].kind, PanelKind::OpenCode);
        assert_eq!(sessions[0].session_id, "session-other");
        assert_eq!(sessions[0].cwd.as_deref(), Some("/other"));
        assert_eq!(sessions[1].session_id, "session-root");
        assert_eq!(sessions[1].cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn parse_pi_session_uses_header_metadata_and_latest_user_message() {
        let jsonl = concat!(
            "{\"type\":\"session\",\"id\":\"pi-session-123\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"first prompt\"}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"working\"}}\n",
            "{\"type\":\"user_message\",\"text\":\"latest prompt\"}\n",
        );

        let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

        assert_eq!(session.kind, PanelKind::Pi);
        assert_eq!(session.session_id, "pi-session-123");
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
        assert_eq!(session.label.as_deref(), Some("latest prompt"));
        assert_eq!(session.updated_at, 42);
    }

    #[test]
    fn parse_pi_session_falls_back_to_filename_id_and_default_label() {
        let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"ok\"}}\n";

        let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

        assert_eq!(session.kind, PanelKind::Pi);
        assert_eq!(session.session_id, "fallback-id");
        assert_eq!(session.cwd, None);
        assert_eq!(session.label.as_deref(), Some("Pi session"));
        assert_eq!(session.updated_at, 7);
    }

    #[test]
    fn parse_pi_session_marks_parent_sessions_noninteractive() {
        let jsonl = "{\"type\":\"session\",\"id\":\"child\",\"parent_id\":\"root\"}\n";

        let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

        assert!(!session.interactive);
    }

    #[test]
    fn load_pi_sessions_recurses_and_filters_by_cwd() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let nested = temp_dir.path().join("project/subdir");
        std::fs::create_dir_all(&nested).expect("create nested session dir");
        std::fs::write(
            nested.join("pi-session-123.jsonl"),
            concat!(
                "{\"session_id\":\"pi-session-123\",\"metadata\":{\"cwd\":\"/repo\"}}\n",
                "{\"role\":\"user\",\"content\":\"Fix the build\"}\n",
            ),
        )
        .expect("write pi session");
        std::fs::write(
            temp_dir.path().join("pi-session-other.jsonl"),
            concat!(
                "{\"session_id\":\"pi-session-other\",\"cwd\":\"/other\"}\n",
                "{\"role\":\"user\",\"content\":\"Other repo\"}\n",
            ),
        )
        .expect("write other pi session");

        let sessions = load_pi_sessions_from_dir(temp_dir.path()).expect("pi sessions");
        let catalog = AgentSessionCatalog { sessions };
        let repo_sessions = catalog.recent_for(PanelKind::Pi, Some("/repo"));

        assert_eq!(repo_sessions.len(), 1);
        assert_eq!(repo_sessions[0].session_id, "pi-session-123");
        assert_eq!(repo_sessions[0].label.as_deref(), Some("Fix the build"));
        assert!(catalog.recent_for(PanelKind::Pi, Some("/missing")).is_empty());
        assert!(catalog.recent_for(PanelKind::Claude, Some("/repo")).is_empty());
    }
}
