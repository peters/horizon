use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::error::{Error, Result};

use super::{AgentSessionRecord, PanelKind, normalize_cwd};

const MAX_PARENT_CANDIDATE_STEPS: usize = 64;
const MAX_TOTAL_PARENT_TRAVERSAL_STEPS: usize = MAX_PARENT_CANDIDATE_STEPS * 2;

#[derive(Clone)]
enum RootResolution {
    Resolved(AgentSessionRecord),
    Rejected,
    CycleDetected,
    BudgetExhausted,
}

impl RootResolution {
    fn unresolved(metadata: &Self, source: &Self) -> Self {
        if matches!(metadata, Self::BudgetExhausted) || matches!(source, Self::BudgetExhausted) {
            Self::BudgetExhausted
        } else if matches!(metadata, Self::CycleDetected) || matches!(source, Self::CycleDetected) {
            Self::CycleDetected
        } else {
            Self::Rejected
        }
    }

    fn is_memoizable(&self) -> bool {
        matches!(self, Self::Resolved(_) | Self::Rejected)
    }
}

struct RootTraversal {
    active: HashSet<String>,
    memoized: HashMap<String, RootResolution>,
    remaining_total_steps: usize,
}

impl RootTraversal {
    fn new() -> Self {
        Self {
            active: HashSet::new(),
            memoized: HashMap::new(),
            remaining_total_steps: MAX_TOTAL_PARENT_TRAVERSAL_STEPS,
        }
    }

    fn resolve_candidate(
        &mut self,
        parent_id: &str,
        expected_cwd: &str,
        threads_by_id: &HashMap<&str, &CodexThread>,
    ) -> RootResolution {
        let remaining_candidate_steps = MAX_PARENT_CANDIDATE_STEPS.min(self.remaining_total_steps);
        self.resolve_parent(parent_id, expected_cwd, threads_by_id, remaining_candidate_steps)
    }

    fn resolve_parent(
        &mut self,
        parent_id: &str,
        expected_cwd: &str,
        threads_by_id: &HashMap<&str, &CodexThread>,
        remaining_candidate_steps: usize,
    ) -> RootResolution {
        let Some(thread) = threads_by_id.get(parent_id) else {
            return RootResolution::Rejected;
        };
        self.resolve_record(thread, expected_cwd, threads_by_id, remaining_candidate_steps)
    }

    fn resolve_record(
        &mut self,
        thread: &CodexThread,
        expected_cwd: &str,
        threads_by_id: &HashMap<&str, &CodexThread>,
        remaining_candidate_steps: usize,
    ) -> RootResolution {
        let thread_id = &thread.record.session_id;
        if let Some(resolution) = self.memoized.get(thread_id) {
            return resolution.clone();
        }
        if self.active.contains(thread_id) {
            return RootResolution::CycleDetected;
        }
        if remaining_candidate_steps == 0 || self.remaining_total_steps == 0 {
            return RootResolution::BudgetExhausted;
        }
        self.remaining_total_steps -= 1;
        let next_candidate_steps = remaining_candidate_steps - 1;

        if !thread.source.is_parent_controlled {
            let resolution = if !thread.archived && thread.record.cwd.as_deref() == Some(expected_cwd) {
                RootResolution::Resolved(thread.record.clone())
            } else {
                RootResolution::Rejected
            };
            self.memoized.insert(thread_id.clone(), resolution.clone());
            return resolution;
        }

        self.active.insert(thread_id.clone());
        let metadata_parent = thread
            .rollout_path
            .as_deref()
            .and_then(|path| rollout_root_session_id(path, thread_id));
        let metadata_resolution = metadata_parent
            .as_deref()
            .map_or(RootResolution::Rejected, |parent_id| {
                self.resolve_parent(parent_id, expected_cwd, threads_by_id, next_candidate_steps)
            });
        let resolution = match metadata_resolution {
            RootResolution::Resolved(root) => RootResolution::Resolved(root),
            metadata_resolution => {
                let source_resolution = match thread.source.parent_thread_id.as_deref() {
                    Some(parent_id) if Some(parent_id) == metadata_parent.as_deref() => metadata_resolution.clone(),
                    Some(parent_id) => {
                        self.resolve_parent(parent_id, expected_cwd, threads_by_id, next_candidate_steps)
                    }
                    None => RootResolution::Rejected,
                };
                match source_resolution {
                    RootResolution::Resolved(root) => RootResolution::Resolved(root),
                    source_resolution => RootResolution::unresolved(&metadata_resolution, &source_resolution),
                }
            }
        };
        self.active.remove(thread_id);
        if resolution.is_memoizable() {
            self.memoized.insert(thread_id.clone(), resolution.clone());
        }
        resolution
    }
}

#[cfg(test)]
std::thread_local! {
    static ROLLOUT_METADATA_READ_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
pub(super) struct CodexSessions {
    pub(super) sessions: Vec<AgentSessionRecord>,
    pub(super) root_aliases: HashMap<String, AgentSessionRecord>,
    pub(super) child_binding_ids: HashSet<String>,
}

struct CodexThread {
    rollout_path: Option<PathBuf>,
    source: CodexThreadSource,
    archived: bool,
    record: AgentSessionRecord,
}

#[derive(Default)]
struct CodexThreadSource {
    is_parent_controlled: bool,
    parent_thread_id: Option<String>,
    malformed: bool,
}

pub(super) fn load_sessions(binding_ids: &HashSet<String>) -> Result<CodexSessions> {
    let Some(home) = user_home_dir() else {
        if !binding_ids.is_empty() {
            return Err(Error::State(
                "cannot validate saved Codex sessions without a user home directory".to_string(),
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

    let mut threads = Vec::new();
    let mut rows = statement.query([]).map_err(|error| Error::State(error.to_string()))?;
    let mut row_number = 0_u64;
    while let Some(row) = rows.next().map_err(|error| Error::State(error.to_string()))? {
        row_number += 1;
        let id = row.get::<_, String>(0).ok().filter(|id| !id.is_empty());
        let source = row.get::<_, String>(2).ok();
        let archived = row.get::<_, bool>(6).ok();
        let updated_at = row.get::<_, i64>(5).ok();
        let (Some(id), Some(source), Some(archived), Some(updated_at)) = (id, source, archived, updated_at) else {
            tracing::warn!(row_number, "skipping malformed Codex thread metadata");
            continue;
        };
        let source = parse_thread_source(&source);
        if source.malformed {
            tracing::warn!(
                row_number,
                thread_id = id,
                "skipping malformed Codex thread source metadata"
            );
            continue;
        }
        let label = (!archived && !source.is_parent_controlled)
            .then(|| row.get::<_, String>(3).ok())
            .flatten()
            .filter(|title| !title.is_empty());
        threads.push(CodexThread {
            rollout_path: row
                .get::<_, String>(1)
                .ok()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            source,
            archived,
            record: AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: id,
                label,
                cwd: normalize_cwd(row.get::<_, String>(4).ok().filter(|cwd| !cwd.is_empty()).as_deref()),
                updated_at: updated_at.saturating_mul(1000),
            },
        });
    }

    let threads_by_id: HashMap<_, _> = threads
        .iter()
        .map(|thread| (thread.record.session_id.as_str(), thread))
        .collect();
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
        .filter(|thread| !thread.archived && !thread.source.is_parent_controlled)
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
                .is_some_and(|thread| thread.source.is_parent_controlled)
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
    if !child.source.is_parent_controlled {
        return None;
    }

    let expected_cwd = child.record.cwd.as_deref()?;
    let metadata_parent = child
        .rollout_path
        .as_deref()
        .and_then(|path| rollout_root_session_id(path, binding_id));
    let source_parent = child.source.parent_thread_id.as_deref();
    let mut traversal = RootTraversal::new();
    let metadata_resolution = metadata_parent
        .as_deref()
        .map_or(RootResolution::Rejected, |parent_id| {
            traversal.resolve_candidate(parent_id, expected_cwd, threads_by_id)
        });
    let root = match metadata_resolution {
        RootResolution::Resolved(root) => root,
        metadata_resolution => {
            let source_resolution = match source_parent {
                Some(parent_id) if Some(parent_id) == metadata_parent.as_deref() => metadata_resolution,
                Some(parent_id) => traversal.resolve_candidate(parent_id, expected_cwd, threads_by_id),
                None => RootResolution::Rejected,
            };
            let RootResolution::Resolved(root) = source_resolution else {
                return None;
            };
            root
        }
    };

    Some((binding_id.to_string(), root))
}

fn rollout_root_session_id(path: &Path, expected_child_id: &str) -> Option<String> {
    const MAX_METADATA_BYTES: u64 = 1024 * 1024;

    #[cfg(test)]
    ROLLOUT_METADATA_READ_COUNT.with(|count| count.set(count.get() + 1));

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
    for line in text.lines() {
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
        let Some(session_id) = payload
            .get("session_id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("parent_thread_id").and_then(Value::as_str))
        else {
            continue;
        };
        if !session_id.is_empty() {
            return Some(session_id.to_owned());
        }
    }
    None
}

#[cfg(test)]
fn reset_rollout_metadata_read_count() {
    ROLLOUT_METADATA_READ_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn rollout_metadata_read_count() -> usize {
    ROLLOUT_METADATA_READ_COUNT.with(std::cell::Cell::get)
}

fn parse_thread_source(source: &str) -> CodexThreadSource {
    let Ok(Value::Object(source)) = serde_json::from_str(source) else {
        return CodexThreadSource {
            malformed: source.trim_start().starts_with('{'),
            ..CodexThreadSource::default()
        };
    };
    let Some(subagent) = source.get("subagent") else {
        return CodexThreadSource::default();
    };
    CodexThreadSource {
        is_parent_controlled: true,
        parent_thread_id: subagent
            .pointer("/thread_spawn/parent_thread_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        malformed: false,
    }
}

fn user_home_dir() -> Option<PathBuf> {
    user_home_dir_from_env(
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    )
}

fn user_home_dir_from_env(home: Option<PathBuf>, user_profile: Option<PathBuf>) -> Option<PathBuf> {
    home.or(user_profile)
}

#[cfg(test)]
mod tests;
