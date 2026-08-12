use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, Row, params};
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::local_store::{codex_db_path, open_read_only_sqlite};

use super::{AgentSessionRecord, PanelKind, normalize_cwd};

const MAX_PARENT_CANDIDATE_STEPS: usize = 64;
const MAX_PARENT_TRAVERSAL_STEPS_PER_BINDING: usize = MAX_PARENT_CANDIDATE_STEPS * 2;
const MAX_MALFORMED_ROW_WARNINGS: usize = 3;
const MAX_ROLLOUT_METADATA_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
enum RootResolution {
    Resolved(AgentSessionRecord),
    Rejected,
    CycleDetected,
    BudgetExhausted,
    Unavailable,
}

impl RootResolution {
    fn unresolved(metadata: &Self, source: &Self) -> Self {
        if matches!(metadata, Self::Unavailable) || matches!(source, Self::Unavailable) {
            Self::Unavailable
        } else if matches!(metadata, Self::BudgetExhausted) || matches!(source, Self::BudgetExhausted) {
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

#[derive(Default)]
pub(super) struct CodexSessions {
    pub(super) sessions: Vec<AgentSessionRecord>,
    pub(super) root_aliases: HashMap<String, AgentSessionRecord>,
    pub(super) child_binding_ids: HashSet<String>,
    pub(super) verified_binding_ids: HashSet<String>,
    pub(super) unavailable_binding_ids: HashSet<String>,
}

#[derive(Clone)]
struct CodexThread {
    rollout_path: Option<PathBuf>,
    source: CodexThreadSource,
    archived: bool,
    record: AgentSessionRecord,
}

#[derive(Clone, Default)]
struct CodexThreadSource {
    is_parent_controlled: bool,
    parent_thread_id: Option<String>,
    malformed: bool,
}

#[derive(Clone)]
enum CachedThread {
    Found(CodexThread),
    Missing,
    Unavailable(String),
}

struct CodexStore {
    connection: Connection,
    threads: HashMap<String, CachedThread>,
}

impl CodexStore {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            threads: HashMap::new(),
        }
    }

    fn thread(&mut self, thread_id: &str) -> Result<CachedThread> {
        if let Some(cached) = self.threads.get(thread_id) {
            return Ok(cached.clone());
        }

        let cached = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT id, rollout_path, source, substr(title, 1, 256), cwd, updated_at, archived
                 FROM threads
                 WHERE id = ?1",
                )
                .map_err(|error| Error::State(format!("failed preparing Codex thread lookup: {error}")))?;
            let mut rows = statement
                .query(params![thread_id])
                .map_err(|error| Error::State(format!("failed querying Codex thread metadata: {error}")))?;
            match rows
                .next()
                .map_err(|error| Error::State(format!("failed reading Codex thread metadata: {error}")))?
            {
                Some(row) => match decode_thread(row) {
                    Ok(thread) => CachedThread::Found(thread),
                    Err(error) => CachedThread::Unavailable(error.to_string()),
                },
                None => CachedThread::Missing,
            }
        };
        self.threads.insert(thread_id.to_string(), cached.clone());
        Ok(cached)
    }
}

struct RootTraversal<'a> {
    store: &'a mut CodexStore,
    rollout_metadata: RolloutMetadataCache,
    memoized: HashMap<(String, String), RootResolution>,
    remaining_binding_steps: usize,
}

impl<'a> RootTraversal<'a> {
    fn new(store: &'a mut CodexStore) -> Self {
        Self {
            store,
            rollout_metadata: RolloutMetadataCache::default(),
            memoized: HashMap::new(),
            remaining_binding_steps: MAX_PARENT_TRAVERSAL_STEPS_PER_BINDING,
        }
    }

    fn binding(&mut self, binding_id: &str) -> Result<BindingResolution> {
        self.remaining_binding_steps = MAX_PARENT_TRAVERSAL_STEPS_PER_BINDING;
        let thread = match self.store.thread(binding_id)? {
            CachedThread::Found(thread) => thread,
            CachedThread::Missing => return Ok(BindingResolution::Unavailable),
            CachedThread::Unavailable(error) => {
                tracing::warn!(thread_id = binding_id, %error, "failed decoding saved Codex thread metadata");
                return Ok(BindingResolution::Unavailable);
            }
        };
        if !thread.source.is_parent_controlled {
            return Ok(if thread.archived {
                BindingResolution::Unavailable
            } else {
                BindingResolution::VerifiedRoot
            });
        }

        let Some(expected_cwd) = thread.record.cwd.as_deref() else {
            return Ok(BindingResolution::Unavailable);
        };
        let metadata_parent = thread
            .rollout_path
            .as_deref()
            .and_then(|path| self.rollout_metadata.parent_id(path, binding_id));
        let source_parent = thread.source.parent_thread_id.as_deref();
        let metadata_resolution = match metadata_parent.as_deref() {
            Some(parent_id) => self.resolve_candidate(parent_id, expected_cwd)?,
            None => RootResolution::Rejected,
        };
        let resolution = match metadata_resolution {
            RootResolution::Resolved(root) => RootResolution::Resolved(root),
            metadata_resolution => {
                let source_resolution = match source_parent {
                    Some(parent_id) if Some(parent_id) == metadata_parent.as_deref() => metadata_resolution.clone(),
                    Some(parent_id) => self.resolve_candidate(parent_id, expected_cwd)?,
                    None => RootResolution::Rejected,
                };
                match source_resolution {
                    RootResolution::Resolved(root) => RootResolution::Resolved(root),
                    source_resolution => RootResolution::unresolved(&metadata_resolution, &source_resolution),
                }
            }
        };
        Ok(match resolution {
            RootResolution::Resolved(root) => BindingResolution::Rebind(root),
            RootResolution::Rejected
            | RootResolution::CycleDetected
            | RootResolution::BudgetExhausted
            | RootResolution::Unavailable => BindingResolution::Unavailable,
        })
    }

    fn resolve_candidate(&mut self, parent_id: &str, expected_cwd: &str) -> Result<RootResolution> {
        let mut active = HashSet::new();
        let remaining_candidate_steps = MAX_PARENT_CANDIDATE_STEPS.min(self.remaining_binding_steps);
        self.resolve_parent(parent_id, expected_cwd, remaining_candidate_steps, &mut active)
    }

    fn resolve_parent(
        &mut self,
        parent_id: &str,
        expected_cwd: &str,
        remaining_candidate_steps: usize,
        active: &mut HashSet<String>,
    ) -> Result<RootResolution> {
        let key = (parent_id.to_string(), expected_cwd.to_string());
        if let Some(resolution) = self.memoized.get(&key) {
            return Ok(resolution.clone());
        }
        if active.contains(parent_id) {
            return Ok(RootResolution::CycleDetected);
        }
        if remaining_candidate_steps == 0 || self.remaining_binding_steps == 0 {
            return Ok(RootResolution::BudgetExhausted);
        }
        let thread = match self.store.thread(parent_id)? {
            CachedThread::Found(thread) => thread,
            CachedThread::Missing => return Ok(RootResolution::Rejected),
            CachedThread::Unavailable(_) => return Ok(RootResolution::Unavailable),
        };
        self.remaining_binding_steps -= 1;
        let next_candidate_steps = remaining_candidate_steps - 1;

        if !thread.source.is_parent_controlled {
            let resolution = if !thread.archived && thread.record.cwd.as_deref() == Some(expected_cwd) {
                RootResolution::Resolved(thread.record)
            } else {
                RootResolution::Rejected
            };
            self.memoized.insert(key, resolution.clone());
            return Ok(resolution);
        }

        if next_candidate_steps == 0 || self.remaining_binding_steps == 0 {
            return Ok(RootResolution::BudgetExhausted);
        }

        active.insert(thread.record.session_id.clone());
        let metadata_parent = thread
            .rollout_path
            .as_deref()
            .and_then(|path| self.rollout_metadata.parent_id(path, &thread.record.session_id));
        let metadata_resolution = match metadata_parent.as_deref() {
            Some(parent_id) => self.resolve_parent(parent_id, expected_cwd, next_candidate_steps, active)?,
            None => RootResolution::Rejected,
        };
        let resolution = match metadata_resolution {
            RootResolution::Resolved(root) => RootResolution::Resolved(root),
            metadata_resolution => {
                let source_resolution = match thread.source.parent_thread_id.as_deref() {
                    Some(parent_id) if Some(parent_id) == metadata_parent.as_deref() => metadata_resolution.clone(),
                    Some(parent_id) => self.resolve_parent(parent_id, expected_cwd, next_candidate_steps, active)?,
                    None => RootResolution::Rejected,
                };
                match source_resolution {
                    RootResolution::Resolved(root) => RootResolution::Resolved(root),
                    source_resolution => RootResolution::unresolved(&metadata_resolution, &source_resolution),
                }
            }
        };
        active.remove(&thread.record.session_id);
        if resolution.is_memoizable() {
            self.memoized.insert(key, resolution.clone());
        }
        Ok(resolution)
    }
}

enum BindingResolution {
    VerifiedRoot,
    Rebind(AgentSessionRecord),
    Unavailable,
}

pub(super) fn load_sessions(binding_ids: &HashSet<String>, include_session_catalog: bool) -> Result<CodexSessions> {
    if binding_ids.is_empty() && !include_session_catalog {
        return Ok(CodexSessions::default());
    }
    let Some(sqlite_path) = codex_db_path() else {
        return if binding_ids.is_empty() {
            Ok(CodexSessions::default())
        } else {
            Err(Error::State(
                "cannot validate saved Codex sessions without a Codex state directory".to_string(),
            ))
        };
    };
    if !sqlite_path.exists() {
        return if binding_ids.is_empty() {
            Ok(CodexSessions::default())
        } else {
            Err(Error::State(format!(
                "cannot validate saved Codex sessions because {} is missing",
                sqlite_path.display()
            )))
        };
    }
    load_sessions_from_path_with_catalog(&sqlite_path, binding_ids, include_session_catalog)
}

fn load_sessions_from_path_with_catalog(
    sqlite_path: &Path,
    binding_ids: &HashSet<String>,
    include_session_catalog: bool,
) -> Result<CodexSessions> {
    if binding_ids.is_empty() && !include_session_catalog {
        return Ok(CodexSessions::default());
    }
    let connection = open_read_only_sqlite(sqlite_path)?;
    let sessions = if include_session_catalog {
        load_active_sessions(&connection)?
    } else {
        Vec::new()
    };
    let mut store = CodexStore::new(connection);
    let mut traversal = RootTraversal::new(&mut store);
    let mut loaded = CodexSessions {
        sessions,
        ..CodexSessions::default()
    };

    let mut sorted_binding_ids: Vec<_> = binding_ids.iter().collect();
    sorted_binding_ids.sort_unstable();
    for binding_id in sorted_binding_ids {
        match traversal.binding(binding_id)? {
            BindingResolution::VerifiedRoot => {
                loaded.verified_binding_ids.insert(binding_id.clone());
            }
            BindingResolution::Rebind(root) => {
                loaded.child_binding_ids.insert(binding_id.clone());
                loaded.root_aliases.insert(binding_id.clone(), root);
            }
            BindingResolution::Unavailable => {
                loaded.child_binding_ids.insert(binding_id.clone());
                loaded.unavailable_binding_ids.insert(binding_id.clone());
            }
        }
    }

    Ok(loaded)
}

fn load_active_sessions(connection: &Connection) -> Result<Vec<AgentSessionRecord>> {
    let mut statement = connection
        .prepare(
            "SELECT id, substr(title, 1, 256), cwd, updated_at
             FROM threads
             WHERE archived = 0
               AND typeof(source) = 'text'
               AND trim(source) IN ('cli', 'vscode', 'exec')
             ORDER BY updated_at DESC",
        )
        .map_err(|error| Error::State(error.to_string()))?;
    let mut rows = statement.query([]).map_err(|error| Error::State(error.to_string()))?;
    let mut sessions = Vec::new();
    let mut malformed_rows = 0usize;
    let mut first_error = None;
    while let Some(row) = rows.next().map_err(|error| Error::State(error.to_string()))? {
        match decode_active_session(row) {
            Ok(session) => sessions.push(session),
            Err(error) => {
                malformed_rows += 1;
                first_error.get_or_insert_with(|| error.to_string());
                if malformed_rows <= MAX_MALFORMED_ROW_WARNINGS {
                    tracing::warn!(row_number = sessions.len() + malformed_rows, %error, "skipping malformed Codex thread metadata");
                }
            }
        }
    }
    if malformed_rows > MAX_MALFORMED_ROW_WARNINGS {
        tracing::warn!(
            malformed_rows,
            reported = MAX_MALFORMED_ROW_WARNINGS,
            "suppressed additional malformed Codex thread warnings"
        );
    }
    if sessions.is_empty()
        && let Some(error) = first_error
    {
        return Err(Error::State(format!("failed decoding Codex thread metadata: {error}")));
    }
    Ok(sessions)
}

fn decode_active_session(row: &Row<'_>) -> rusqlite::Result<AgentSessionRecord> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let cwd: String = row.get(2)?;
    let updated_at: i64 = row.get(3)?;
    Ok(AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: id,
        label: (!title.is_empty()).then_some(title),
        cwd: normalize_cwd((!cwd.is_empty()).then_some(cwd.as_str())),
        updated_at: updated_at.saturating_mul(1000),
        interactive: true,
    })
}

fn decode_thread(row: &Row<'_>) -> rusqlite::Result<CodexThread> {
    let id: String = row.get(0)?;
    let rollout_path: String = row.get(1)?;
    let source: String = row.get(2)?;
    let title: String = row.get(3)?;
    let cwd: String = row.get(4)?;
    let updated_at: i64 = row.get(5)?;
    let archived: bool = row.get(6)?;
    let source = parse_thread_source(&source);
    if source.malformed {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            "malformed Codex thread source metadata".into(),
        ));
    }
    Ok(CodexThread {
        rollout_path: (!rollout_path.is_empty()).then(|| PathBuf::from(rollout_path)),
        source: source.clone(),
        archived,
        record: AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: id,
            label: (!archived && !source.is_parent_controlled && !title.is_empty()).then_some(title),
            cwd: normalize_cwd((!cwd.is_empty()).then_some(cwd.as_str())),
            updated_at: updated_at.saturating_mul(1000),
            interactive: !source.is_parent_controlled,
        },
    })
}

#[derive(Clone, Deserialize)]
struct RolloutMetadata {
    id: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    parent_thread_id: Option<String>,
}

#[derive(Deserialize)]
struct RolloutLine {
    #[serde(rename = "type")]
    record_type: String,
    payload: Option<RolloutMetadata>,
}

#[derive(Default)]
struct RolloutMetadataCache {
    entries: HashMap<PathBuf, Option<RolloutMetadata>>,
}

impl RolloutMetadataCache {
    fn parent_id(&mut self, path: &Path, expected_child_id: &str) -> Option<String> {
        let metadata = self
            .entries
            .entry(path.to_path_buf())
            .or_insert_with(|| read_rollout_metadata(path))
            .as_ref()?;
        if metadata.id != expected_child_id {
            return None;
        }
        metadata
            .session_id
            .as_deref()
            .filter(|session_id| !session_id.is_empty() && *session_id != expected_child_id)
            .or_else(|| {
                metadata
                    .parent_thread_id
                    .as_deref()
                    .filter(|parent_id| !parent_id.is_empty() && *parent_id != expected_child_id)
            })
            .map(str::to_owned)
    }
}

fn read_rollout_metadata(path: &Path) -> Option<RolloutMetadata> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut reader = BufReader::new(file).take(MAX_ROLLOUT_METADATA_BYTES);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let Ok(record) = serde_json::from_str::<RolloutLine>(&line) else {
            continue;
        };
        if record.record_type == "session_meta" {
            return record.payload;
        }
    }
}

fn parse_thread_source(source: &str) -> CodexThreadSource {
    let source = match serde_json::from_str(source) {
        Ok(Value::Object(source)) => source,
        Err(_) if matches!(source.trim(), "cli" | "vscode" | "exec") => {
            return CodexThreadSource::default();
        }
        Ok(_) | Err(_) => {
            return CodexThreadSource {
                malformed: true,
                ..CodexThreadSource::default()
            };
        }
    };
    let Some(subagent) = source.get("subagent").filter(|subagent| !subagent.is_null()) else {
        return CodexThreadSource {
            malformed: true,
            ..CodexThreadSource::default()
        };
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

#[cfg(test)]
mod tests;
