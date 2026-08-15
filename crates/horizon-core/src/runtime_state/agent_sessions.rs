use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::local_store::open_read_only_sqlite;
use crate::opencode_paths::opencode_db_path;
use crate::text::flatten_and_truncate_chars;

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
    Stale,
    Unavailable,
}

pub struct AgentSessionBootstrapCatalog {
    catalog: AgentSessionCatalog,
    exact_resolutions: HashMap<(PanelKind, String), ExactSessionResolution>,
    unavailable_exact_session_ids: HashSet<String>,
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

    /// Load provider catalogs needed to repair, assign, or manually rebind the
    /// panels present at startup.
    ///
    /// # Errors
    ///
    /// Returns an error when a provider declares exact-session validation but
    /// has no validator implementation, or when an existing Codex store cannot
    /// validate saved exact IDs. A missing supported Codex store classifies
    /// those IDs as stale so startup can safely fall back to fresh launches;
    /// optional provider discovery remains best-effort.
    pub fn load_for_runtime_state(runtime_state: &RuntimeState) -> Result<AgentSessionBootstrapCatalog> {
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
        let has_claude_panels = Self::has_provider_panel(runtime_state, PanelKind::Claude);
        let has_codex_panels = Self::has_provider_panel(runtime_state, PanelKind::Codex);
        let has_opencode_panels = Self::has_provider_panel(runtime_state, PanelKind::OpenCode);
        let has_pi_panels = Self::has_provider_panel(runtime_state, PanelKind::Pi);

        let mut sessions = Vec::new();
        if has_claude_panels {
            extend_best_effort(&mut sessions, "Claude", load_claude_sessions());
        }
        if has_opencode_panels {
            extend_best_effort(&mut sessions, "OpenCode", load_opencode_sessions());
        }
        if has_pi_panels {
            extend_best_effort(&mut sessions, "Pi", load_pi_sessions());
        }
        let mut codex_binding_ids = HashSet::new();
        for (kind, binding_ids) in exact_binding_ids {
            match kind {
                PanelKind::Codex => codex_binding_ids = binding_ids,
                unsupported => {
                    return Err(Error::State(format!(
                        "no exact-session validator is implemented for {}",
                        unsupported.display_name()
                    )));
                }
            }
        }
        // Exact Codex validation also seeds the root-only rebind menu. Include
        // the active catalog whenever the board has a Codex panel so pinned
        // panels still have manual alternatives.
        let include_active_codex_sessions = has_codex_panels || !codex_binding_ids.is_empty();
        let codex = finish_codex_load(
            codex::load_sessions(&codex_binding_ids, include_active_codex_sessions),
            &codex_binding_ids,
        )?;
        Ok(AgentSessionBootstrapCatalog::new(
            Self::from_provider_sessions(sessions, &codex),
            codex,
        ))
    }

    fn has_provider_panel(runtime_state: &RuntimeState, kind: PanelKind) -> bool {
        runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .any(|panel| panel.kind == kind)
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
        let unavailable_exact_session_ids = codex.unavailable_binding_ids.clone();
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
        exact_resolutions.extend(
            codex
                .stale_binding_ids
                .into_iter()
                .map(|session_id| ((PanelKind::Codex, session_id), ExactSessionResolution::Stale)),
        );
        Self {
            catalog,
            exact_resolutions,
            unavailable_exact_session_ids,
        }
    }

    #[must_use]
    pub fn unavailable_exact_session_ids(&self) -> &HashSet<String> {
        &self.unavailable_exact_session_ids
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

fn finish_codex_load(
    loaded: Result<codex::CodexSessions>,
    exact_binding_ids: &HashSet<String>,
) -> Result<codex::CodexSessions> {
    match loaded {
        Ok(codex) => Ok(codex),
        Err(error) if exact_binding_ids.is_empty() => {
            tracing::warn!("failed loading optional Codex sessions: {error}");
            Ok(codex::CodexSessions::default())
        }
        Err(error) => Err(error),
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
    parent_controlled: Option<bool>,
}

impl ClaudeSessionSummary {
    fn apply_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }

        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };

        if self.parent_controlled.is_none() {
            self.parent_controlled = Some(value.get("isSidechain").and_then(Value::as_bool) == Some(true));
        }

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
            self.last_prompt = Some(truncate_discovered_session_label(found_prompt));
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
            interactive: !self.parent_controlled.unwrap_or(false),
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

fn truncate_discovered_session_label(value: &str) -> String {
    const MAX_CHARS: usize = 64;

    flatten_and_truncate_chars(value.trim(), MAX_CHARS).into_owned()
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

        if value.get("type").and_then(Value::as_str) == Some("session") {
            self.parent_controlled |=
                string_field(&value, &["parentSession", "parent_session"]).is_some_and(|parent| !parent.is_empty());
        }

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
            self.last_user_message = Some(truncate_discovered_session_label(&user_message));
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
mod tests;
