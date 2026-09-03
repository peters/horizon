//! Explicit resume policy for interrupted deterministic plan runs.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Plan, PlanStep, StepReport};

/// Verified completions and the optional in-flight step for one job.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCheckpoint {
    /// Tool results persisted only after MCP returned a structured outcome.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed: Vec<StepReport>,
    /// Step whose mutation may have been dispatched but is not yet verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<CheckpointIntent>,
    /// Uncertain steps the operator chose not to replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

impl RunCheckpoint {
    /// True when no completions, intent, or skips have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.completed.is_empty() && self.intent.is_none() && self.skipped.is_empty()
    }
}

/// Intent recorded before an MCP tool future is polled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointIntent {
    /// Plan step that is about to run, or that was interrupted in flight.
    pub step_id: String,
    /// MCP tool name for diagnostics.
    pub tool: String,
    /// Whether the runner observed a dispatch or only a crash-safe intent.
    pub status: IntentStatus,
}

/// Lifecycle of a persisted intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    /// Intent is durable; the MCP call may not have been polled yet.
    Dispatched,
    /// The call was interrupted after dispatch, so replay is unsafe by default.
    Uncertain,
}

/// How `resume` treats an uncertain in-flight mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UncertainPolicy {
    /// Refuse to resume until the operator inspects the browser audit.
    #[default]
    Fail,
    /// Leave the uncertain step unreplayed and continue with later steps.
    Skip,
}

/// Prefix to reuse and the next plan index to execute.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeSelection {
    /// Verified step reports reused for `$ref` resolution.
    pub completed: Vec<StepReport>,
    /// First plan index that is still eligible to run.
    pub start_index: usize,
    /// Uncertain step skipped by explicit policy, if any.
    pub skipped: Option<String>,
}

/// Durable persistence of intent and verified completions.
pub trait CheckpointStore {
    /// Persist intent before the MCP tool future is polled.
    ///
    /// # Errors
    /// Returns when the durable checkpoint cannot be written.
    fn record_intent(&mut self, step: &PlanStep) -> Result<(), String>;
    /// Persist a verified success or fail-fast tool outcome.
    ///
    /// # Errors
    /// Returns when the durable checkpoint cannot be written.
    fn record_completion(&mut self, report: &StepReport) -> Result<(), String>;
    /// Mark the in-flight step uncertain after a dispatched interrupt.
    ///
    /// # Errors
    /// Returns when the durable checkpoint cannot be written.
    fn record_uncertain(&mut self, step: &PlanStep) -> Result<(), String>;
    /// Drop a not-yet-polled intent so resume can retry that step.
    ///
    /// # Errors
    /// Returns when the durable checkpoint cannot be written.
    fn clear_intent(&mut self) -> Result<(), String>;
}

/// Failure to choose a safe resume point.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ResumeError {
    /// The job id is not a published `job-` identifier.
    #[error("invalid job id `{0}`")]
    InvalidJobId(String),
    /// No published job directory exists for this id.
    #[error("durable job `{0}` was not found")]
    NotFound(String),
    /// The saved plan or state could not be decoded.
    #[error("{0}")]
    Decode(String),
    /// Prepared state has not expired, so another runner may still own the job.
    #[error("durable job `{0}` may still be running; automatic continuation is disabled")]
    StillRunning(String),
    /// The job already reached a successful terminal report.
    #[error("durable job `{0}` already succeeded")]
    AlreadySucceeded(String),
    /// Every plan step is already verified or skipped.
    #[error("durable job `{0}` has no remaining steps to resume")]
    NothingToResume(String),
    /// An interrupted mutation must not be replayed without an explicit skip.
    #[error(
        "durable job has uncertain step `{step}` (`{tool}`); pass --on-uncertain skip after inspecting the browser audit"
    )]
    Uncertain { step: String, tool: String },
    /// Saved completions do not match the saved plan prefix.
    #[error("durable checkpoint does not match the saved plan prefix")]
    PrefixMismatch,
    /// The job was recorded before checkpoints existed.
    #[error("durable job `{0}` was recorded before checkpoints; resume would replay completed mutations")]
    LegacyState(String),
    /// The original run or another resume already holds this job.
    #[error("durable job `{0}` is already running or being resumed")]
    Locked(String),
}

impl UncertainPolicy {
    /// Parse the `--on-uncertain` CLI value.
    ///
    /// # Errors
    /// Returns when the value is not `fail` or `skip`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "fail" => Ok(Self::Fail),
            "skip" => Ok(Self::Skip),
            _ => Err("--on-uncertain requires fail or skip".to_string()),
        }
    }
}

/// Choose the next runnable index without replaying verified or uncertain work.
///
/// # Errors
/// Returns when the checkpoint does not match the plan or an uncertain step
/// would be replayed under the fail policy.
pub fn select_resume(
    plan: &Plan,
    checkpoint: Option<&RunCheckpoint>,
    policy: UncertainPolicy,
) -> Result<ResumeSelection, ResumeError> {
    let checkpoint = checkpoint.cloned().unwrap_or_default();
    let reports = reports_by_id(&checkpoint.completed)?;
    let skipped_ids = checkpoint.skipped.iter().cloned().collect::<BTreeSet<_>>();
    let mut completed = Vec::new();
    let mut start_index = 0;
    let mut skip_intent = None;
    for (index, step) in plan.steps.iter().enumerate() {
        if let Some(report) = reports.get(step.id.as_str()) {
            if report.tool != step.tool {
                return Err(ResumeError::PrefixMismatch);
            }
            completed.push((*report).clone());
            start_index = index.saturating_add(1);
            continue;
        }
        if skipped_ids.contains(&step.id) {
            start_index = index.saturating_add(1);
            continue;
        }
        if let Some(intent) = checkpoint.intent.as_ref() {
            if intent.step_id != step.id {
                return Err(ResumeError::PrefixMismatch);
            }
            match policy {
                UncertainPolicy::Fail => {
                    return Err(ResumeError::Uncertain {
                        step: intent.step_id.clone(),
                        tool: intent.tool.clone(),
                    });
                }
                UncertainPolicy::Skip => {
                    skip_intent = Some(intent.step_id.clone());
                    start_index = index.saturating_add(1);
                }
            }
        }
        break;
    }
    Ok(ResumeSelection {
        completed,
        start_index,
        skipped: skip_intent,
    })
}

fn reports_by_id(completed: &[StepReport]) -> Result<BTreeMap<&str, &StepReport>, ResumeError> {
    let mut reports = BTreeMap::new();
    for report in completed {
        if reports.insert(report.id.as_str(), report).is_some() {
            return Err(ResumeError::PrefixMismatch);
        }
    }
    Ok(reports)
}

/// True when `job-` is followed by a UUID and no path separators.
#[must_use]
pub fn valid_job_id(job_id: &str) -> bool {
    let Some(suffix) = job_id.strip_prefix("job-") else {
        return false;
    };
    uuid::Uuid::parse_str(suffix).is_ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::PlanStep;

    fn plan() -> Plan {
        Plan {
            version: 1,
            steps: vec![
                step("list", "browser_list"),
                step("navigate", "browser_navigate"),
                step("title", "browser_evaluate"),
            ],
        }
    }

    fn step(id: &str, tool: &str) -> PlanStep {
        PlanStep {
            id: id.to_string(),
            tool: tool.to_string(),
            arguments: serde_json::Map::new(),
        }
    }

    fn completed(id: &str, tool: &str) -> StepReport {
        StepReport {
            id: id.to_string(),
            tool: tool.to_string(),
            ok: true,
            result: Some(json!({"ok": true})),
            error: None,
        }
    }

    #[test]
    fn resume_continues_after_verified_completions() {
        let checkpoint = RunCheckpoint {
            completed: vec![completed("list", "browser_list")],
            intent: None,
            skipped: Vec::new(),
        };
        let selection = select_resume(&plan(), Some(&checkpoint), UncertainPolicy::Fail).expect("resume");
        assert_eq!(selection.start_index, 1);
        assert_eq!(selection.completed.len(), 1);
        assert!(selection.skipped.is_none());
    }

    #[test]
    fn uncertain_intent_is_not_replayed_by_default() {
        let checkpoint = RunCheckpoint {
            completed: vec![completed("list", "browser_list")],
            intent: Some(CheckpointIntent {
                step_id: "navigate".to_string(),
                tool: "browser_navigate".to_string(),
                status: IntentStatus::Uncertain,
            }),
            skipped: Vec::new(),
        };
        let error = select_resume(&plan(), Some(&checkpoint), UncertainPolicy::Fail).expect_err("uncertain");
        assert!(matches!(error, ResumeError::Uncertain { step, .. } if step == "navigate"));
    }

    #[test]
    fn skip_policy_advances_past_the_uncertain_step() {
        let checkpoint = RunCheckpoint {
            completed: vec![completed("list", "browser_list")],
            intent: Some(CheckpointIntent {
                step_id: "navigate".to_string(),
                tool: "browser_navigate".to_string(),
                status: IntentStatus::Dispatched,
            }),
            skipped: Vec::new(),
        };
        let selection = select_resume(&plan(), Some(&checkpoint), UncertainPolicy::Skip).expect("skip");
        assert_eq!(selection.start_index, 2);
        assert_eq!(selection.skipped.as_deref(), Some("navigate"));
    }

    #[test]
    fn leftover_dispatched_intent_is_uncertain_on_resume() {
        let checkpoint = RunCheckpoint {
            completed: Vec::new(),
            intent: Some(CheckpointIntent {
                step_id: "list".to_string(),
                tool: "browser_list".to_string(),
                status: IntentStatus::Dispatched,
            }),
            skipped: Vec::new(),
        };
        let error = select_resume(&plan(), Some(&checkpoint), UncertainPolicy::Fail).expect_err("dispatched");
        assert!(matches!(error, ResumeError::Uncertain { step, .. } if step == "list"));
    }

    #[test]
    fn skipped_steps_are_merged_in_plan_order() {
        let checkpoint = RunCheckpoint {
            completed: vec![
                completed("list", "browser_list"),
                completed("title", "browser_evaluate"),
            ],
            intent: None,
            skipped: vec!["navigate".to_string()],
        };
        let selection = select_resume(&plan(), Some(&checkpoint), UncertainPolicy::Fail).expect("merge");
        assert_eq!(selection.start_index, 3);
        assert_eq!(
            selection
                .completed
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            ["list", "title"]
        );
    }

    #[test]
    fn job_ids_reject_path_traversal() {
        assert!(valid_job_id("job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4"));
        assert!(!valid_job_id("../job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4"));
        assert!(!valid_job_id("job-not-a-uuid"));
        assert_eq!(
            ResumeError::Locked("job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4".to_string()).to_string(),
            "durable job `job-4e212c23-d0dd-4ae2-bf69-9ec08fdad2b4` is already running or being resumed"
        );
    }
}
