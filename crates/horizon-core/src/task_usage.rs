use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::Board;
use crate::panel::PanelKind;
use crate::task::TaskUsageSummary;

#[must_use]
pub fn collect_task_usage(board: &Board) -> Vec<TaskUsageSummary> {
    let mut usage_by_task: BTreeMap<String, TaskUsageSummary> = BTreeMap::new();
    let codex_usage = load_codex_usage().unwrap_or_default();

    for workspace in &board.workspaces {
        let Some(binding) = &workspace.task_binding else {
            continue;
        };

        let mut summary = TaskUsageSummary {
            task_id: binding.task_id.clone(),
            label: binding.label(),
            claude_sessions: 0,
            claude_tokens: 0,
            claude_messages: 0,
            codex_sessions: 0,
            codex_tokens: 0,
        };
        let mut seen_claude = BTreeSet::new();
        let mut seen_codex = BTreeSet::new();

        for panel_id in &workspace.panels {
            let Some(panel) = board.panel(*panel_id) else {
                continue;
            };
            let Some(session_binding) = &panel.session_binding else {
                continue;
            };

            match panel.kind {
                PanelKind::Claude if seen_claude.insert(session_binding.session_id.clone()) => {
                    let (tokens, messages) =
                        load_claude_session_usage(&session_binding.session_id, session_binding.cwd.as_deref());
                    summary.claude_sessions = summary.claude_sessions.saturating_add(1);
                    summary.claude_tokens = summary.claude_tokens.saturating_add(tokens);
                    summary.claude_messages = summary.claude_messages.saturating_add(messages);
                }
                PanelKind::Codex if seen_codex.insert(session_binding.session_id.clone()) => {
                    if let Some(tokens) = codex_usage.get(&session_binding.session_id) {
                        summary.codex_tokens = summary.codex_tokens.saturating_add(*tokens);
                    }
                    summary.codex_sessions = summary.codex_sessions.saturating_add(1);
                }
                _ => {}
            }
        }

        usage_by_task.insert(summary.task_id.clone(), summary);
    }

    usage_by_task.into_values().collect()
}

fn load_codex_usage() -> Option<BTreeMap<String, u64>> {
    let sqlite_path = std::env::var_os("HOME")
        .map(PathBuf::from)?
        .join(".codex/state_5.sqlite");
    if !sqlite_path.exists() {
        return None;
    }

    let connection = Connection::open(sqlite_path).ok()?;
    let mut statement = connection
        .prepare("SELECT id, tokens_used FROM threads WHERE archived = 0")
        .ok()?;
    let rows = statement
        .query_map(params![], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .ok()?;

    let mut usage = BTreeMap::new();
    for row in rows.flatten() {
        usage.insert(row.0, u64::try_from(row.1).unwrap_or_default());
    }
    Some(usage)
}

fn load_claude_session_usage(session_id: &str, cwd: Option<&str>) -> (u64, u32) {
    let Some(path) = claude_session_path(session_id, cwd) else {
        return (0, 0);
    };
    let Ok(file) = std::fs::File::open(path) else {
        return (0, 0);
    };

    let mut total_tokens = 0u64;
    let mut assistant_messages = 0u32;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        assistant_messages = assistant_messages.saturating_add(1);
        let Some(usage) = value.get("message").and_then(|message| message.get("usage")) else {
            continue;
        };
        total_tokens = total_tokens
            .saturating_add(usage.get("input_tokens").and_then(Value::as_u64).unwrap_or_default())
            .saturating_add(
                usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            )
            .saturating_add(
                usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            )
            .saturating_add(usage.get("output_tokens").and_then(Value::as_u64).unwrap_or_default());
    }

    (total_tokens, assistant_messages)
}

fn claude_session_path(session_id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cwd = cwd?;
    let project_dir = format!("-{}", cwd.trim_start_matches('/').replace('/', "-"));
    let direct = home
        .join(".claude/projects")
        .join(project_dir)
        .join(format!("{session_id}.jsonl"));
    if direct.exists() {
        return Some(direct);
    }

    find_claude_session_fallback(&home.join(".claude/projects"), session_id)
}

fn find_claude_session_fallback(root: &std::path::Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = entry.file_type().ok()?;
        if file_type.is_dir() {
            if path.file_name().and_then(std::ffi::OsStr::to_str) == Some("subagents") {
                continue;
            }
            if let Some(found) = find_claude_session_fallback(&path, session_id) {
                return Some(found);
            }
        } else if path.file_stem().and_then(std::ffi::OsStr::to_str) == Some(session_id) {
            return Some(path);
        }
    }
    None
}
