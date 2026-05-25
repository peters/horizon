use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::panel::PanelKind;

pub const AGENT_SQUAD_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentSquad {
    pub version: u32,
    pub runs: Vec<SquadRun>,
}

impl AgentSquad {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load an Agent Squad file if one exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or decoded.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        let mut squad = serde_json::from_str::<Self>(&content).map_err(|error| Error::State(error.to_string()))?;
        squad.version = AGENT_SQUAD_VERSION;
        Ok(squad)
    }

    /// Serialize this Agent Squad state as stable pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|error| Error::State(error.to_string()))
    }

    pub fn create_run(&mut self, goal: impl Into<String>, created_at_millis: i64) -> &mut SquadRun {
        self.runs.push(SquadRun::new(new_id(), goal, created_at_millis));
        let index = self.runs.len() - 1;
        &mut self.runs[index]
    }

    /// Return a mutable run by id.
    ///
    /// # Errors
    ///
    /// Returns an error if no run with `run_id` exists.
    pub fn run_mut(&mut self, run_id: &str) -> Result<&mut SquadRun> {
        self.runs
            .iter_mut()
            .find(|run| run.id == run_id)
            .ok_or_else(|| Error::State(format!("squad run {run_id} was not found")))
    }
}

impl Default for AgentSquad {
    fn default() -> Self {
        Self {
            version: AGENT_SQUAD_VERSION,
            runs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SquadRun {
    pub id: String,
    pub goal: String,
    pub created_at_millis: i64,
    pub status: RunStatus,
    pub researcher: Option<AgentPanelLink>,
    pub reviewer: Option<AgentPanelLink>,
    pub performers: Vec<PerformerSlot>,
    pub isolation: IsolationMode,
    pub primary_worktree: Option<PathBuf>,
    pub plan_text: String,
    pub failure_reason: Option<String>,
}

impl SquadRun {
    #[must_use]
    pub fn new(id: impl Into<String>, goal: impl Into<String>, created_at_millis: i64) -> Self {
        Self {
            id: id.into(),
            goal: goal.into(),
            created_at_millis,
            ..Self::default()
        }
    }

    pub fn start_decomposing(&mut self, researcher: AgentPanelLink) {
        self.researcher = Some(researcher);
        self.status = RunStatus::Decomposing;
    }

    pub fn queue_plan(&mut self, plan_text: impl Into<String>, performers: Vec<PerformerSlot>) {
        self.plan_text = plan_text.into();
        self.performers = performers;
        self.status = RunStatus::FanningOut;
    }

    pub fn set_primary_worktree(&mut self, path: PathBuf) {
        self.primary_worktree = Some(path);
    }

    /// Mark a performer slot as dispatched to a panel.
    ///
    /// # Errors
    ///
    /// Returns an error if no slot with `slot_id` exists.
    pub fn dispatch_slot(&mut self, slot_id: &str, panel_local_id: impl Into<String>) -> Result<()> {
        let slot = self.slot_mut(slot_id)?;
        slot.panel_local_id = Some(panel_local_id.into());
        slot.work_item.status = WorkStatus::Dispatched;
        self.status = RunStatus::Working;
        Ok(())
    }

    /// Mark a performer slot as done and store its report.
    ///
    /// # Errors
    ///
    /// Returns an error if no slot with `slot_id` exists.
    pub fn mark_slot_done(&mut self, slot_id: &str, report: PerformerReport) -> Result<()> {
        let slot = self.slot_mut(slot_id)?;
        slot.report = Some(report);
        slot.work_item.status = WorkStatus::Done;
        self.refresh_working_status();
        Ok(())
    }

    /// Mark a performer slot as blocked with reviewer follow-up context.
    ///
    /// # Errors
    ///
    /// Returns an error if no slot with `slot_id` exists.
    pub fn mark_slot_blocked(&mut self, slot_id: &str, follow_up: impl Into<String>) -> Result<()> {
        let slot = self.slot_mut(slot_id)?;
        let follow_up = follow_up.into();
        slot.report = Some(PerformerReport {
            summary: "Blocked".to_string(),
            follow_up,
            ..PerformerReport::default()
        });
        slot.work_item.status = WorkStatus::Blocked;
        self.refresh_working_status();
        Ok(())
    }

    /// Move a run into review and bind its reviewer panel.
    ///
    /// # Errors
    ///
    /// Returns an error if any performer slot is not done.
    pub fn start_reviewing(&mut self, reviewer: AgentPanelLink) -> Result<()> {
        if self
            .performers
            .iter()
            .any(|slot| slot.work_item.status != WorkStatus::Done)
        {
            return Err(Error::State(format!(
                "squad run {} cannot start review until every slot is done",
                self.id
            )));
        }

        self.reviewer = Some(reviewer);
        self.status = RunStatus::Reviewing;
        Ok(())
    }

    pub fn finish(&mut self) {
        self.status = RunStatus::Done;
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        self.failure_reason = Some(reason.into());
        self.status = RunStatus::Failed;
    }

    fn slot_mut(&mut self, slot_id: &str) -> Result<&mut PerformerSlot> {
        self.performers
            .iter_mut()
            .find(|slot| slot.id == slot_id)
            .ok_or_else(|| Error::State(format!("performer slot {slot_id} was not found")))
    }

    fn refresh_working_status(&mut self) {
        if self.performers.is_empty() {
            return;
        }
        if self.status == RunStatus::Reviewing || self.status == RunStatus::Done || self.status == RunStatus::Failed {
            return;
        }
        if self
            .performers
            .iter()
            .all(|slot| slot.work_item.status == WorkStatus::Done)
        {
            self.status = RunStatus::Working;
        }
    }
}

impl Default for SquadRun {
    fn default() -> Self {
        Self {
            id: String::new(),
            goal: String::new(),
            created_at_millis: 0,
            status: RunStatus::Draft,
            researcher: None,
            reviewer: None,
            performers: Vec::new(),
            isolation: IsolationMode::Worktree,
            primary_worktree: None,
            plan_text: String::new(),
            failure_reason: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PerformerSlot {
    pub id: String,
    pub work_item: WorkItem,
    pub assigned_kind: PanelKind,
    pub panel_local_id: Option<String>,
    pub scratch: PathBuf,
    pub report: Option<PerformerReport>,
}

impl PerformerSlot {
    #[must_use]
    pub fn new(id: impl Into<String>, work_item: WorkItem, assigned_kind: PanelKind, scratch: PathBuf) -> Self {
        Self {
            id: id.into(),
            work_item,
            assigned_kind,
            panel_local_id: None,
            scratch,
            report: None,
        }
    }
}

impl Default for PerformerSlot {
    fn default() -> Self {
        Self {
            id: String::new(),
            work_item: WorkItem::default(),
            assigned_kind: PanelKind::Codex,
            panel_local_id: None,
            scratch: PathBuf::new(),
            report: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkItem {
    pub id: String,
    pub title: String,
    pub request: String,
    pub acceptance_criteria: Vec<String>,
    pub suggested_commands: Vec<String>,
    pub status: WorkStatus,
}

impl WorkItem {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>, request: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            request: request.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PerformerReport {
    pub summary: String,
    pub validation_commands: Vec<String>,
    pub validation_result: String,
    pub follow_up: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentPanelLink {
    pub kind: PanelKind,
    pub panel_local_id: Option<String>,
}

impl AgentPanelLink {
    #[must_use]
    pub fn new(kind: PanelKind, panel_local_id: Option<String>) -> Self {
        Self { kind, panel_local_id }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[default]
    Draft,
    Decomposing,
    FanningOut,
    Working,
    Reviewing,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    #[default]
    Queued,
    Dispatched,
    Done,
    Blocked,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    #[default]
    Worktree,
    TmpDir,
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn missing_squad_file_loads_empty_model() {
        let dir = TempDir::new().unwrap();

        let squad = AgentSquad::load(&dir.path().join("squad.json")).unwrap();

        assert_eq!(squad.version, AGENT_SQUAD_VERSION);
        assert!(squad.runs.is_empty());
    }

    #[test]
    fn squad_model_round_trips_as_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("squad.json");
        let mut squad = AgentSquad::new();
        let run = squad.create_run("Find perf wins", 42);
        run.start_decomposing(AgentPanelLink::new(PanelKind::Claude, Some("panel-r".to_string())));
        run.queue_plan("Two tasks", vec![slot("s1"), slot("s2")]);

        fs::write(&path, squad.to_json().unwrap()).unwrap();
        let loaded = AgentSquad::load(&path).unwrap();

        assert_eq!(loaded, squad);
    }

    #[test]
    fn run_transitions_from_research_to_review() {
        let mut run = SquadRun::new("run-1", "Fix issues", 123);

        run.start_decomposing(AgentPanelLink::new(PanelKind::Claude, Some("panel-r".to_string())));
        assert_eq!(run.status, RunStatus::Decomposing);

        run.queue_plan("Plan", vec![slot("s1"), slot("s2")]);
        assert_eq!(run.status, RunStatus::FanningOut);

        run.dispatch_slot("s1", "panel-1").unwrap();
        run.dispatch_slot("s2", "panel-2").unwrap();
        assert_eq!(run.status, RunStatus::Working);

        run.mark_slot_done("s1", report("s1 done")).unwrap();
        assert!(
            run.start_reviewing(AgentPanelLink::new(PanelKind::Claude, None))
                .is_err()
        );

        run.mark_slot_done("s2", report("s2 done")).unwrap();
        run.start_reviewing(AgentPanelLink::new(PanelKind::Claude, Some("panel-review".to_string())))
            .unwrap();
        assert_eq!(run.status, RunStatus::Reviewing);

        run.finish();
        assert_eq!(run.status, RunStatus::Done);
    }

    #[test]
    fn blocked_slot_prevents_review() {
        let mut run = SquadRun::new("run-1", "Fix issues", 123);
        run.queue_plan("Plan", vec![slot("s1")]);
        run.dispatch_slot("s1", "panel-1").unwrap();

        run.mark_slot_blocked("s1", "needs fixture").unwrap();
        let error = run
            .start_reviewing(AgentPanelLink::new(PanelKind::Claude, Some("panel-review".to_string())))
            .unwrap_err();

        assert!(error.to_string().contains("cannot start review"));
        assert_eq!(run.performers[0].work_item.status, WorkStatus::Blocked);
        assert_eq!(run.status, RunStatus::Working);
    }

    fn slot(id: &str) -> PerformerSlot {
        PerformerSlot::new(
            id,
            WorkItem::new(id, format!("Task {id}"), "Do the thing"),
            PanelKind::Codex,
            PathBuf::from(format!("/tmp/squad/{id}")),
        )
    }

    fn report(summary: &str) -> PerformerReport {
        PerformerReport {
            summary: summary.to_string(),
            validation_commands: vec!["cargo test".to_string()],
            validation_result: "pass".to_string(),
            follow_up: String::new(),
        }
    }
}
