//! Mission orchestration domain types and the `plan.json` schema.
//!
//! A mission is a frontier-model-produced plan of self-contained tasks that
//! Horizon executes with local worker agents. The plan file is the only
//! machine interface between the planner (an agent panel) and Horizon: the
//! planner writes `plan.json`, Horizon watches and validates it, and never
//! parses model chatter.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Bumped when the `plan.json` shape changes incompatibly.
pub const MISSION_PLAN_SCHEMA: u32 = 1;

/// Upper bound on tasks a single mission plan may carry.
pub const MAX_MISSION_TASKS: usize = 32;

/// Minimum plan revision. Refines bump it monotonically.
pub const FIRST_PLAN_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    /// Create a task id, rejecting malformed ids up front.
    ///
    /// # Errors
    ///
    /// Fails when the id is not 2-8 alphanumeric characters starting with a letter.
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let id = Self(raw.into());
        validate_task_id(&id)?;
        Ok(id)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Coarse size class of a task; drives token estimates and scheduling weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl TaskSize {
    /// Baseline worker token estimate in millions, before the thinking
    /// multiplier. Matches the mission cost model used by the UI.
    #[must_use]
    pub const fn base_megatokens(self) -> f32 {
        match self {
            Self::Small => 0.3,
            Self::Medium => 0.6,
            Self::Large => 0.9,
        }
    }
}

/// Worker thinking level, passed through to `pi --thinking <level>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    /// Token multiplier applied to the size baseline (thinking tokens are
    /// free on self-hosted workers but still count toward the estimate).
    #[must_use]
    pub const fn effort_factor(self) -> f32 {
        match self {
            Self::Off => 1.0,
            Self::Minimal => 1.1,
            Self::Low => 1.25,
            Self::Medium => 1.5,
            Self::High => 2.0,
            Self::Xhigh => 2.8,
            Self::Max => 3.5,
        }
    }

    /// Value of the `--thinking` flag; `None` when the flag is omitted.
    #[must_use]
    pub fn flag_value(self) -> Option<&'static str> {
        (self != Self::Off).then_some(match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        })
    }
}

impl fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.flag_value().unwrap_or("off"))
    }
}

/// Which model family runs a task: the mission's self-hosted worker profile
/// (default) or the frontier model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskModel {
    #[default]
    Worker,
    Frontier,
}

/// Planner backend for a mission. The planner spends frontier tokens once,
/// up front, and only ever writes plan files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerKind {
    /// `codex` CLI in planner mode.
    CodexCli,
    /// `pi` pointed at a frontier model.
    Pi,
}

/// Lifecycle of the mission as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    /// Planner panel is running; no (valid) plan yet.
    Planning,
    /// A validated plan exists; mutable until execution starts.
    Planned,
    /// Workers are running; the plan is locked except for live refinements.
    Running,
    /// All selected tasks finished.
    Complete,
    /// Abandoned by the user; worktrees may be pruned.
    Discarded,
}

/// Lifecycle of a single task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Done,
    Failed,
    /// The task was re-planned away; `by` names the replacement task.
    /// Treated as terminal for dependency and progress purposes.
    Replaced {
        by: TaskId,
    },
    /// Deselected before execution; never spawned.
    Skipped,
}

impl TaskStatus {
    /// Terminal statuses that satisfy dependency edges.
    #[must_use]
    pub fn satisfies_deps(self) -> bool {
        matches!(self, Self::Done | Self::Replaced { .. })
    }

    /// Terminal statuses that count toward mission completion.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Replaced { .. } | Self::Skipped)
    }
}

/// One self-contained unit of work in a mission plan.
///
/// `brief` must be complete on its own: workers start without the planning
/// conversation, so every assumption the planner made has to live here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: TaskId,
    pub title: String,
    pub brief: String,
    pub size: TaskSize,
    /// Task ids that must finish (or be replaced) first.
    pub deps: Vec<TaskId>,
    /// Git worktree name isolating this worker's edits.
    pub worktree: String,
    pub model: TaskModel,
    pub effort: ThinkingLevel,
    #[serde(default = "default_selected")]
    pub selected: bool,
    /// Plan version in which this task was introduced.
    pub version: u32,
    pub status: TaskStatus,
}

fn default_selected() -> bool {
    true
}

/// The parsed and validated shape of a `plan.json` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionPlan {
    pub schema: u32,
    pub version: u32,
    pub tasks: Vec<PlanTask>,
}

/// Mission metadata persisted next to the plan (`mission.json`): identity,
/// goal, planner choice, and the mission-level status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionMeta {
    pub id: String,
    pub goal: String,
    pub planner: PlannerKind,
    pub status: MissionStatus,
}

impl MissionPlan {
    /// Validate a plan: ids, deps, cycles, briefs, worktree names, and
    /// replacement consistency. Call this on every load so a hand-edited or
    /// hallucinated plan can never wedge the scheduler.
    ///
    /// # Errors
    ///
    /// Fails on unknown or duplicate ids, cycles, empty titles/briefs, bad
    /// worktree names, or inconsistent replacement links.
    pub fn validate(&self) -> Result<()> {
        if self.version < FIRST_PLAN_VERSION {
            return Err(crate::Error::Mission(format!(
                "plan version {} is below the first version {FIRST_PLAN_VERSION}",
                self.version
            )));
        }
        if self.tasks.len() > MAX_MISSION_TASKS {
            return Err(crate::Error::Mission(format!(
                "plan has {} tasks, limit is {MAX_MISSION_TASKS}",
                self.tasks.len()
            )));
        }

        let mut seen = std::collections::HashSet::new();
        for task in &self.tasks {
            validate_task_id(&task.id)?;
            if !seen.insert(task.id.clone()) {
                return Err(crate::Error::Mission(format!("duplicate task id `{}`", task.id)));
            }
        }

        for task in &self.tasks {
            let id = task.id.to_string();
            if task.title.trim().is_empty() {
                return Err(crate::Error::Mission(format!("task {id} has an empty title")));
            }
            if task.brief.trim().is_empty() {
                return Err(crate::Error::Mission(format!(
                    "task {id} has an empty brief; workers need a self-contained prompt"
                )));
            }
            validate_worktree_name(&task.worktree, &id)?;
            for dep in &task.deps {
                if dep == &task.id {
                    return Err(crate::Error::Mission(format!("task {id} depends on itself")));
                }
                if !seen.contains(dep) {
                    return Err(crate::Error::Mission(format!(
                        "task {id} depends on unknown task `{dep}`"
                    )));
                }
            }
            if let TaskStatus::Replaced { by } = &task.status {
                if by == &task.id {
                    return Err(crate::Error::Mission(format!("task {id} is replaced by itself")));
                }
                if !seen.contains(by) {
                    return Err(crate::Error::Mission(format!(
                        "task {id} was replaced by unknown task `{by}`"
                    )));
                }
            }
        }

        ensure_acyclic(&self.tasks)
    }
}

impl MissionMeta {
    /// Mission ids become directory names under `.horizon/missions/`.
    ///
    /// # Errors
    ///
    /// Fails when the id is not a safe slug or the goal is empty.
    pub fn validate_id(&self) -> Result<()> {
        let id = self.id.as_str();
        if !is_valid_slug(id) {
            return Err(crate::Error::Mission(format!(
                "mission id `{id}` must match [a-z0-9][a-z0-9-]* (1..=32 chars)"
            )));
        }
        if self.goal.trim().is_empty() {
            return Err(crate::Error::Mission("mission goal is empty".into()));
        }
        Ok(())
    }
}

fn is_valid_slug(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    (1..=32).contains(&value.len()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate_task_id(id: &TaskId) -> Result<()> {
    let raw = id.as_str();
    let valid = (2..=8).contains(&raw.len()) && raw.chars().all(|c| c.is_ascii_alphabetic() || c.is_ascii_digit());
    if !valid || !raw.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return Err(crate::Error::Mission(format!(
            "task id `{raw}` must be 2-8 alphanumeric chars starting with a letter"
        )));
    }
    Ok(())
}

fn validate_worktree_name(name: &str, task_id: &str) -> Result<()> {
    let valid = (1..=60).contains(&name.len())
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Err(crate::Error::Mission(format!(
            "task {task_id} has invalid worktree name `{name}`; use [a-z0-9_-], 1-60 chars, not starting with -"
        )));
    }
    Ok(())
}

fn ensure_acyclic(tasks: &[PlanTask]) -> Result<()> {
    let index: std::collections::HashMap<&TaskId, usize> = tasks.iter().enumerate().map(|(i, t)| (&t.id, i)).collect();
    let mut color = vec![0u8; tasks.len()]; // 0 unvisited, 1 in progress, 2 done
    for start in 0..tasks.len() {
        if color[start] == 0 {
            dfs_acyclic(start, tasks, &index, &mut color)?;
        }
    }
    Ok(())
}

fn dfs_acyclic(
    node: usize,
    tasks: &[PlanTask],
    index: &std::collections::HashMap<&TaskId, usize>,
    color: &mut [u8],
) -> Result<()> {
    color[node] = 1;
    for dep in &tasks[node].deps {
        let Some(&next) = index.get(dep) else {
            continue; // unknown deps are reported by the id pass
        };
        match color[next] {
            1 => {
                return Err(crate::Error::Mission(format!(
                    "dependency cycle involving `{}` and `{}`",
                    tasks[node].id, tasks[next].id
                )));
            }
            0 => dfs_acyclic(next, tasks, index, color)?,
            _ => {}
        }
    }
    color[node] = 2;
    Ok(())
}

/// Read and validate `plan.json`. A stale or hand-corrupted file is an error,
/// never a silent partial plan.
///
/// # Errors
///
/// Fails on I/O problems, unparseable JSON, a schema mismatch, or any
/// validation failure in [`MissionPlan::validate`].
pub fn load_plan_file(path: &Path) -> Result<MissionPlan> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Mission(format!("reading plan file {}: {e}", path.display())))?;
    let plan: MissionPlan = serde_json::from_str(&raw)
        .map_err(|e| crate::Error::Mission(format!("parsing plan file {}: {e}", path.display())))?;
    if plan.schema != MISSION_PLAN_SCHEMA {
        return Err(crate::Error::Mission(format!(
            "plan file {} uses schema {}, expected {MISSION_PLAN_SCHEMA}",
            path.display(),
            plan.schema
        )));
    }
    plan.validate()?;
    Ok(plan)
}

/// Write `plan.json` atomically-enough for the watcher (write, then sync).
///
/// # Errors
///
/// Fails on serialization problems or I/O errors writing the file.
pub fn save_plan_file(path: &Path, plan: &MissionPlan) -> Result<()> {
    let mut raw =
        serde_json::to_string_pretty(plan).map_err(|e| crate::Error::Mission(format!("serializing plan: {e}")))?;
    raw.push('\n');
    std::fs::write(path, raw).map_err(|e| crate::Error::Mission(format!("writing plan file {}: {e}", path.display())))
}

/// Read the mission metadata file (`mission.json`).
///
/// # Errors
///
/// Fails on I/O problems, unparseable JSON, or an invalid mission id/goal.
pub fn load_mission_file(path: &Path) -> Result<MissionMeta> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| crate::Error::Mission(format!("reading mission file {}: {e}", path.display())))?;
    let meta: MissionMeta = serde_json::from_str(&raw)
        .map_err(|e| crate::Error::Mission(format!("parsing mission file {}: {e}", path.display())))?;
    meta.validate_id()?;
    Ok(meta)
}

/// Write the mission metadata file (`mission.json`).
///
/// # Errors
///
/// Fails on serialization problems or I/O errors writing the file.
pub fn save_mission_file(path: &Path, meta: &MissionMeta) -> Result<()> {
    let mut raw =
        serde_json::to_string_pretty(meta).map_err(|e| crate::Error::Mission(format!("serializing mission: {e}")))?;
    raw.push('\n');
    std::fs::write(path, raw)
        .map_err(|e| crate::Error::Mission(format!("writing mission file {}: {e}", path.display())))
}

#[cfg(test)]
mod tests;
