use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;
use serde_json::Value;

use crate::error::{Error, Result};

use super::{AgentSessionBinding, PanelKind, normalize_cwd};

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
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use uuid::Uuid;

    use super::{
        AgentSessionCatalog, AgentSessionRecord, ClaudeSessionSummary, PanelKind, load_claude_project_session_summary,
        scan_claude_session_reader,
    };
    use crate::panel::PanelResume;
    use crate::runtime_state::{PanelState, RuntimeState, WorkspaceState};

    fn parse_claude_project_session<R: std::io::BufRead>(
        reader: R,
        fallback_session_id: &str,
        fallback_updated_at: i64,
    ) -> Option<AgentSessionRecord> {
        let mut summary = ClaudeSessionSummary::default();
        scan_claude_session_reader(reader, None, &mut summary);
        summary.into_record(fallback_session_id, fallback_updated_at)
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
