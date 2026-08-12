use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::error::{Error, Result};

use super::{AgentSessionRecord, PanelKind, normalize_cwd};

#[derive(Default)]
pub(super) struct CodexSessions {
    pub(super) sessions: Vec<AgentSessionRecord>,
    pub(super) root_aliases: HashMap<String, AgentSessionRecord>,
    pub(super) child_binding_ids: HashSet<String>,
}

struct CodexThread {
    id: String,
    rollout_path: PathBuf,
    source: String,
    archived: bool,
    record: AgentSessionRecord,
}

pub(super) fn load_sessions(binding_ids: &HashSet<String>) -> Result<CodexSessions> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        if !binding_ids.is_empty() {
            return Err(Error::State(
                "cannot validate saved Codex sessions without HOME".to_string(),
            ));
        }
        return Ok(CodexSessions::default());
    };
    let sqlite_path = home.join(".codex/state_5.sqlite");
    if !sqlite_path.exists() {
        if !binding_ids.is_empty() {
            return Err(Error::State(format!(
                "cannot validate saved Codex sessions because {} is missing",
                sqlite_path.display()
            )));
        }
        return Ok(CodexSessions::default());
    }
    load_sessions_from_path(&sqlite_path, binding_ids)
}

fn load_sessions_from_path(sqlite_path: &Path, binding_ids: &HashSet<String>) -> Result<CodexSessions> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection =
        Connection::open_with_flags(sqlite_path, flags).map_err(|error| Error::State(error.to_string()))?;
    let mut statement = connection
        .prepare(
            "SELECT id, rollout_path, source, title, cwd, updated_at, archived
             FROM threads
             ORDER BY updated_at DESC",
        )
        .map_err(|error| Error::State(error.to_string()))?;

    let rows = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let source: String = row.get(2)?;
            let archived: bool = row.get(6)?;
            let label = (!archived && !is_subagent_source(&source))
                .then(|| row.get::<_, String>(3).ok())
                .flatten()
                .filter(|title| !title.is_empty());
            Ok(CodexThread {
                record: AgentSessionRecord {
                    kind: PanelKind::Codex,
                    session_id: id.clone(),
                    label,
                    cwd: normalize_cwd(row.get::<_, String>(4).ok().as_deref()),
                    updated_at: row.get::<_, i64>(5)?.saturating_mul(1000),
                },
                id,
                rollout_path: PathBuf::from(row.get::<_, String>(1)?),
                source,
                archived,
            })
        })
        .map_err(|error| Error::State(error.to_string()))?;

    let mut threads = Vec::new();
    for row in rows {
        threads.push(row.map_err(|error| Error::State(error.to_string()))?);
    }

    let threads_by_id: HashMap<_, _> = threads.iter().map(|thread| (thread.id.as_str(), thread)).collect();
    if let Some(missing_id) = binding_ids
        .iter()
        .find(|binding_id| !threads_by_id.contains_key(binding_id.as_str()))
    {
        return Err(Error::State(format!(
            "saved Codex session {missing_id} is missing from {}",
            sqlite_path.display()
        )));
    }
    let sessions = threads
        .iter()
        .filter(|thread| !thread.archived && !is_subagent_source(&thread.source))
        .map(|thread| thread.record.clone())
        .collect();
    let root_aliases = binding_ids
        .iter()
        .filter_map(|binding_id| resolve_root_alias(binding_id, &threads_by_id))
        .collect();
    let child_binding_ids = binding_ids
        .iter()
        .filter(|binding_id| {
            threads_by_id
                .get(binding_id.as_str())
                .is_some_and(|thread| is_subagent_source(&thread.source))
        })
        .cloned()
        .collect();

    Ok(CodexSessions {
        sessions,
        root_aliases,
        child_binding_ids,
    })
}

fn resolve_root_alias(
    binding_id: &str,
    threads_by_id: &HashMap<&str, &CodexThread>,
) -> Option<(String, AgentSessionRecord)> {
    let child = threads_by_id.get(binding_id)?;
    if !is_subagent_source(&child.source) {
        return None;
    }

    let root_id = rollout_root_session_id(&child.rollout_path, binding_id)
        .filter(|root_id| root_id != binding_id)
        .or_else(|| root_session_id_from_source_chain(child, threads_by_id))?;
    let root = threads_by_id.get(root_id.as_str())?;
    if root.archived || is_subagent_source(&root.source) || child.record.cwd != root.record.cwd {
        return None;
    }

    Some((binding_id.to_string(), root.record.clone()))
}

fn rollout_root_session_id(path: &Path, expected_child_id: &str) -> Option<String> {
    const MAX_METADATA_BYTES: u64 = 64 * 1024;

    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().take(8) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("id").and_then(Value::as_str) != Some(expected_child_id) {
            continue;
        }
        let Some(session_id) = payload.get("session_id").and_then(Value::as_str) else {
            continue;
        };
        if !session_id.is_empty() {
            return Some(session_id.to_owned());
        }
    }
    None
}

fn root_session_id_from_source_chain(
    child: &CodexThread,
    threads_by_id: &HashMap<&str, &CodexThread>,
) -> Option<String> {
    let mut current = child;
    let mut visited = HashSet::new();
    while is_subagent_source(&current.source) {
        if !visited.insert(current.id.as_str()) {
            return None;
        }
        let source = serde_json::from_str::<Value>(&current.source).ok()?;
        let parent_id = source
            .pointer("/subagent/thread_spawn/parent_thread_id")?
            .as_str()?
            .to_string();
        current = threads_by_id.get(parent_id.as_str())?;
    }
    Some(current.id.clone())
}

fn is_subagent_source(source: &str) -> bool {
    let Ok(Value::Object(source)) = serde_json::from_str(source) else {
        return false;
    };
    source.contains_key("subagent")
}

#[cfg(test)]
mod tests;
