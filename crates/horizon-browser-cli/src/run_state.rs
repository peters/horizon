//! Durable metadata for deterministic browser plan runs.

use std::fs::OpenOptions;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use atomicwrites::{AllowOverwrite, AtomicFile};
use horizon_core::HorizonHome;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::checkpoint::{
    CheckpointIntent, CheckpointStore, IntentStatus, ResumeError, ResumeSelection, RunCheckpoint, UncertainPolicy,
    select_resume, valid_job_id,
};
use crate::{
    ExecutionReport, Plan, PlanStep, StepReport,
    execution_control::{BlockingIoError, BlockingIoMode, ExecutionControl, ExecutionStopReason},
};

const STATE_VERSION: u32 = 3;
const MIN_RESUME_VERSION: u32 = 3;
const PLAN_FILE: &str = "plan.json";
const REPORT_FILE: &str = "report.json";
const STATE_FILE: &str = "state.json";
const RESUME_LOCK_FILE: &str = "resume.lock";

/// Persisted lifecycle state for one deterministic plan run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunState {
    /// State format version.
    pub version: u32,
    /// Stable identifier and private directory name for this run.
    pub job_id: String,
    /// Current terminal or non-terminal lifecycle state.
    pub status: RunStatus,
    /// Unix timestamp in milliseconds when the job was created.
    pub created_at_millis: u64,
    /// Configured action budget spanning durable setup and MCP work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_seconds: Option<u64>,
    /// Absolute wall-clock expiry paired with the monotonic job deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at_millis: Option<u64>,
    /// Unix timestamp in milliseconds when this state was last persisted.
    pub updated_at_millis: u64,
    /// Process that initially executed the job. This is diagnostic only.
    pub runner_pid: u32,
    /// Job-directory-relative saved plan path.
    pub plan_file: String,
    /// Job-directory-relative report path after execution produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_file: Option<String>,
    /// Number of step reports produced before the run stopped.
    pub completed_steps: usize,
    /// Verified completions and any in-flight or skipped uncertain step.
    #[serde(default, skip_serializing_if = "RunCheckpoint::is_empty")]
    pub checkpoint: RunCheckpoint,
    /// Private initialization, plan-execution, or MCP shutdown error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Durable plan-run lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Legacy state written before durable preparation became deadline-aware.
    Running,
    /// Initial artifacts are durable and MCP work may run until the recorded
    /// deadline. Once it expires, this state resolves conservatively to
    /// [`Self::TimedOut`] without another write.
    Prepared,
    /// Every plan step and MCP shutdown completed successfully.
    Succeeded,
    /// A plan step, MCP connection, or runner operation failed.
    Failed,
    /// The runner received an explicit cancellation request.
    Cancelled,
    /// The configured job deadline elapsed.
    TimedOut,
}

impl RunState {
    /// Resolve a durable snapshot at a wall-clock instant.
    ///
    /// Prepared state acts as a bounded lease: it may represent an active
    /// runner before expiry, but it is always timed out at or after expiry.
    #[must_use]
    pub fn effective_status_at(&self, unix_millis: u64) -> RunStatus {
        match (self.status, self.deadline_at_millis) {
            (RunStatus::Prepared, Some(deadline)) if unix_millis >= deadline => RunStatus::TimedOut,
            (RunStatus::Prepared, Some(_)) => RunStatus::Running,
            (RunStatus::Prepared, None) => RunStatus::TimedOut,
            (status, _) => status,
        }
    }
}

/// CLI report envelope that makes the durable job discoverable to callers.
#[derive(Serialize)]
pub struct DurableExecutionReport<'a> {
    /// Stable id of the durable run.
    pub job_id: &'a str,
    /// JSON-safe display path to the private job directory.
    pub job_dir: String,
    /// JSON-safe display path to the atomic lifecycle state.
    pub state_path: String,
    /// Existing versioned plan-execution report.
    #[serde(flatten)]
    pub execution: &'a ExecutionReport,
}

/// Owner of the private files for one deterministic plan run.
#[derive(Debug)]
pub struct DurableRun {
    directory: PathBuf,
    state_path: PathBuf,
    state: RunState,
    lock_file: Option<std::fs::File>,
}

/// Detached finalization view of an already published durable run.
#[derive(Debug)]
pub struct DurablePostProcessor {
    run: DurableRun,
}

/// Failure while persisting a checkpoint under execution control.
#[derive(Debug, Error)]
pub enum CheckpointPersistError {
    #[error("{0}")]
    Io(String),
    #[error("{}", .0.message())]
    Stopped(ExecutionStopReason),
}

/// Failure to create or atomically update durable plan-run state.
#[derive(Debug, Error)]
pub enum RunStateError {
    #[error("{operation}: {source}")]
    Io {
        operation: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not encode durable job {artifact}: {source}")]
    Encode {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

/// Preparation failure that retains a job published before its final barrier.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct DurablePreparationError {
    run: Option<Box<DurableRun>>,
    #[source]
    source: RunStateError,
}

impl DurablePreparationError {
    fn unpublished(source: RunStateError) -> Self {
        Self { run: None, source }
    }

    fn published(run: DurableRun, source: RunStateError) -> Self {
        Self {
            run: Some(Box::new(run)),
            source,
        }
    }

    /// Split the recoverable run handle from the underlying failure.
    #[must_use]
    pub fn into_parts(self) -> (Option<DurableRun>, RunStateError) {
        (self.run.map(|run| *run), self.source)
    }
}

impl DurableRun {
    /// Create a private job directory and persist a deadline-bound prepared
    /// state plus the validated plan before any MCP action is attempted.
    ///
    /// # Errors
    /// Returns when the private directory or either initial artifact cannot be
    /// created durably.
    pub fn prepare(
        plan: &Plan,
        execution_timeout_seconds: u64,
        deadline_at_millis: u64,
    ) -> Result<Self, RunStateError> {
        Self::prepare_cancellable(plan, execution_timeout_seconds, deadline_at_millis)
            .map_err(|error| error.into_parts().1)
    }

    /// Prepare a run while retaining a handle if the final publication barrier
    /// fails after the atomic directory rename.
    ///
    /// # Errors
    /// Returns the underlying preparation error and any already published run.
    pub fn prepare_cancellable(
        plan: &Plan,
        execution_timeout_seconds: u64,
        deadline_at_millis: u64,
    ) -> Result<Self, DurablePreparationError> {
        let root = HorizonHome::resolve().root().join("browser-jobs");
        Self::prepare_cancellable_in(&root, plan, Some(execution_timeout_seconds), Some(deadline_at_millis))
    }

    /// Stable id of this durable run.
    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.state.job_id
    }

    /// Private lifecycle-state file for this durable run.
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Clone only the published state needed for detached persistence.
    #[must_use]
    pub fn postprocessor(&self) -> DurablePostProcessor {
        DurablePostProcessor {
            run: Self {
                directory: self.directory.clone(),
                state_path: self.state_path.clone(),
                state: self.state.clone(),
                lock_file: None,
            },
        }
    }

    #[cfg(test)]
    fn prepare_in(
        root: &Path,
        plan: &Plan,
        execution_timeout_seconds: Option<u64>,
        deadline_at_millis: Option<u64>,
    ) -> Result<Self, RunStateError> {
        Self::prepare_cancellable_in(root, plan, execution_timeout_seconds, deadline_at_millis)
            .map_err(|error| error.into_parts().1)
    }

    fn prepare_cancellable_in(
        root: &Path,
        plan: &Plan,
        execution_timeout_seconds: Option<u64>,
        deadline_at_millis: Option<u64>,
    ) -> Result<Self, DurablePreparationError> {
        ensure_private_directory(root).map_err(DurablePreparationError::unpublished)?;
        let job_id = format!("job-{}", Uuid::new_v4());
        let directory = root.join(&job_id);
        let staging = tempfile::Builder::new()
            .prefix(".preparing-")
            .tempdir_in(root)
            .map_err(|source| io_error(format!("could not stage {job_id}"), source))
            .map_err(DurablePreparationError::unpublished)?;
        secure_directory(staging.path()).map_err(DurablePreparationError::unpublished)?;
        let created_at_millis = now_millis();
        let state = RunState {
            version: STATE_VERSION,
            job_id,
            status: RunStatus::Prepared,
            created_at_millis,
            execution_timeout_seconds,
            deadline_at_millis,
            updated_at_millis: created_at_millis,
            runner_pid: std::process::id(),
            plan_file: PLAN_FILE.to_string(),
            report_file: None,
            completed_steps: 0,
            checkpoint: RunCheckpoint::default(),
            error: None,
        };
        write_private_json(&staging.path().join(PLAN_FILE), plan, "plan")
            .map_err(DurablePreparationError::unpublished)?;
        write_private_json(&staging.path().join(STATE_FILE), &state, "state")
            .map_err(DurablePreparationError::unpublished)?;
        sync_directory(staging.path()).map_err(DurablePreparationError::unpublished)?;
        publish_job_directory(staging, &directory).map_err(DurablePreparationError::unpublished)?;
        let mut run = Self {
            state_path: directory.join(STATE_FILE),
            directory,
            state,
            lock_file: None,
        };
        if let Err(source) = sync_directory(root) {
            return Err(DurablePreparationError::published(run, source));
        }
        if let Err(error) = run.acquire_resume_lock() {
            return Err(DurablePreparationError::published(
                run,
                io_error(
                    error.to_string(),
                    std::io::Error::from(std::io::ErrorKind::AlreadyExists),
                ),
            ));
        }
        Ok(run)
    }

    /// Return the report envelope exposed on stdout or through `--output`.
    #[must_use]
    pub fn report<'a>(&'a self, execution: &'a ExecutionReport) -> DurableExecutionReport<'a> {
        DurableExecutionReport {
            job_id: &self.state.job_id,
            job_dir: self.directory.display().to_string(),
            state_path: self.state_path.display().to_string(),
            execution,
        }
    }

    /// Persist the private report and terminal status after plan execution.
    ///
    /// # Errors
    /// Returns when either terminal artifact cannot be atomically persisted.
    pub fn finish(&mut self, execution: &ExecutionReport) -> Result<(), RunStateError> {
        self.write_json(REPORT_FILE, &self.report(execution), "report")?;
        self.state.status = match execution.stop_reason {
            Some(ExecutionStopReason::Cancelled) => RunStatus::Cancelled,
            Some(ExecutionStopReason::DeadlineExceeded) => RunStatus::TimedOut,
            None if execution.ok => RunStatus::Succeeded,
            None => RunStatus::Failed,
        };
        self.state.updated_at_millis = now_millis();
        self.state.report_file = Some(REPORT_FILE.to_string());
        self.state.completed_steps = execution.completed_steps;
        self.state.error.clone_from(&execution.error);
        self.persist_state()
    }

    /// Persist a terminal initialization failure that produced no execution
    /// report.
    ///
    /// # Errors
    /// Returns when the failed state cannot be atomically persisted.
    pub fn fail(&mut self, error: &str) -> Result<(), RunStateError> {
        self.state.status = RunStatus::Failed;
        self.state.updated_at_millis = now_millis();
        self.state.error = Some(error.to_string());
        self.persist_state()
    }

    /// Persist a terminal stop that happened before a step report existed.
    ///
    /// # Errors
    /// Returns when the stopped state cannot be atomically persisted.
    pub fn stop(&mut self, reason: ExecutionStopReason) -> Result<(), RunStateError> {
        self.state.status = match reason {
            ExecutionStopReason::Cancelled => RunStatus::Cancelled,
            ExecutionStopReason::DeadlineExceeded => RunStatus::TimedOut,
        };
        self.state.updated_at_millis = now_millis();
        self.state.error = Some(reason.message().to_string());
        self.persist_state()
    }

    /// Open a previously published job by id.
    ///
    /// # Errors
    /// Returns when the id is invalid, the job is missing, or state cannot be read.
    pub fn open(job_id: &str) -> Result<Self, ResumeError> {
        let root = HorizonHome::resolve().root().join("browser-jobs");
        Self::open_in(&root, job_id)
    }

    pub(crate) fn open_in(root: &Path, job_id: &str) -> Result<Self, ResumeError> {
        if !valid_job_id(job_id) {
            return Err(ResumeError::InvalidJobId(job_id.to_string()));
        }
        let directory = root.join(job_id);
        let state_path = directory.join(STATE_FILE);
        let bytes = std::fs::read(&state_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ResumeError::NotFound(job_id.to_string())
            } else {
                ResumeError::Decode(format!("could not read {}: {source}", state_path.display()))
            }
        })?;
        let state: RunState = serde_json::from_slice(&bytes)
            .map_err(|source| ResumeError::Decode(format!("could not decode {}: {source}", state_path.display())))?;
        if state.job_id != job_id {
            return Err(ResumeError::Decode(format!(
                "durable job `{job_id}` does not match state id `{}`",
                state.job_id
            )));
        }
        Ok(Self {
            directory,
            state_path,
            state,
            lock_file: None,
        })
    }

    /// Load a terminal job and choose remaining work for an explicit resume.
    ///
    /// # Errors
    /// Returns when the job is missing, still leased, already succeeded, or
    /// would replay an uncertain mutation.
    pub fn prepare_resume(job_id: &str, policy: UncertainPolicy) -> Result<(Self, Plan, ResumeSelection), ResumeError> {
        let mut run = Self::open(job_id)?;
        run.acquire_resume_lock()?;
        if run.state.version < MIN_RESUME_VERSION {
            return Err(ResumeError::LegacyState(job_id.to_string()));
        }
        let plan = run.load_plan()?;
        match run.state.effective_status_at(now_millis()) {
            RunStatus::Prepared | RunStatus::Running => return Err(ResumeError::StillRunning(job_id.to_string())),
            RunStatus::Succeeded => return Err(ResumeError::AlreadySucceeded(job_id.to_string())),
            RunStatus::Failed | RunStatus::Cancelled | RunStatus::TimedOut => {}
        }
        let selection = select_resume(&plan, Some(&run.state.checkpoint), policy)?;
        if selection.start_index >= plan.steps.len() {
            return Err(ResumeError::NothingToResume(job_id.to_string()));
        }
        Ok((run, plan, selection))
    }

    /// Saved plan for this durable run.
    ///
    /// # Errors
    /// Returns when the plan file cannot be read or validated.
    pub fn load_plan(&self) -> Result<Plan, ResumeError> {
        let path = self.directory.join(&self.state.plan_file);
        let bytes = std::fs::read(&path)
            .map_err(|source| ResumeError::Decode(format!("could not read {}: {source}", path.display())))?;
        Plan::from_slice(&bytes).map_err(|source| ResumeError::Decode(source.to_string()))
    }

    /// Current durable snapshot.
    #[must_use]
    pub fn state(&self) -> &RunState {
        &self.state
    }

    /// Re-arm a terminal job for an explicit resume without creating a new id.
    ///
    /// # Errors
    /// Returns when the updated lease cannot be persisted.
    pub fn rearm(
        &mut self,
        execution_timeout_seconds: u64,
        deadline_at_millis: u64,
        skipped: Option<String>,
    ) -> Result<(), RunStateError> {
        self.state.status = RunStatus::Prepared;
        self.state.execution_timeout_seconds = Some(execution_timeout_seconds);
        self.state.deadline_at_millis = Some(deadline_at_millis);
        self.state.updated_at_millis = now_millis();
        self.state.runner_pid = std::process::id();
        self.state.error = None;
        if let Some(step_id) = skipped {
            self.state.checkpoint.skipped.push(step_id);
            self.state.checkpoint.intent = None;
        }
        self.persist_state()
    }

    fn persist_state(&self) -> Result<(), RunStateError> {
        write_private_json(&self.state_path, &self.state, "state")?;
        sync_directory(&self.directory)
    }

    async fn persist_checkpoint_controlled(
        &mut self,
        control: &mut ExecutionControl,
        honor_deadline: bool,
        previous: RunCheckpoint,
    ) -> Result<(), CheckpointPersistError> {
        self.state.updated_at_millis = now_millis();
        self.state.completed_steps = self.state.checkpoint.completed.len();
        let state = self.state.clone();
        let state_path = self.state_path.clone();
        let directory = self.directory.clone();
        match control
            .wait_owned_blocking(
                "horizon-browser-checkpoint",
                if honor_deadline {
                    BlockingIoMode::Bound
                } else {
                    BlockingIoMode::Required
                },
                move || write_private_json(&state_path, &state, "state").and_then(|()| sync_directory(&directory)),
            )
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.restore_checkpoint(previous);
                Err(CheckpointPersistError::Io(error.to_string()))
            }
            Err(BlockingIoError::Failed(error)) => {
                self.restore_checkpoint(previous);
                Err(CheckpointPersistError::Io(error))
            }
            Err(BlockingIoError::Stopped(reason)) => {
                self.restore_checkpoint(previous);
                Err(CheckpointPersistError::Stopped(reason))
            }
        }
    }

    fn restore_checkpoint(&mut self, previous: RunCheckpoint) {
        self.state.checkpoint = previous;
        self.state.completed_steps = self.state.checkpoint.completed.len();
    }

    /// Record intent on an owned I/O worker before the MCP future is polled.
    ///
    /// # Errors
    /// Returns when the durable write fails or a stop is observed first.
    pub async fn record_intent_controlled(
        &mut self,
        step: &PlanStep,
        control: &mut ExecutionControl,
    ) -> Result<(), CheckpointPersistError> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = Some(CheckpointIntent {
            step_id: step.id.clone(),
            tool: step.tool.clone(),
            status: IntentStatus::Dispatched,
        });
        self.persist_checkpoint_controlled(control, true, previous).await
    }

    /// Record a verified completion on an owned I/O worker.
    ///
    /// Uses required I/O so a racing stop cannot roll back a structured MCP
    /// outcome that already reached disk.
    ///
    /// # Errors
    /// Returns when the durable write fails.
    pub async fn record_completion_controlled(
        &mut self,
        report: &StepReport,
        control: &mut ExecutionControl,
    ) -> Result<(), CheckpointPersistError> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = None;
        self.state.checkpoint.completed.push(report.clone());
        self.persist_checkpoint_controlled(control, false, previous).await
    }

    /// Mark an in-flight step uncertain without requiring a live deadline.
    ///
    /// # Errors
    /// Returns when the durable write fails or cancellation wins first.
    pub async fn record_uncertain_controlled(
        &mut self,
        step: &PlanStep,
        control: &mut ExecutionControl,
    ) -> Result<(), CheckpointPersistError> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = Some(CheckpointIntent {
            step_id: step.id.clone(),
            tool: step.tool.clone(),
            status: IntentStatus::Uncertain,
        });
        self.persist_checkpoint_controlled(control, false, previous).await
    }

    /// Drop a not-yet-polled intent without requiring a live deadline.
    ///
    /// # Errors
    /// Returns when the durable write fails or cancellation wins first.
    pub async fn clear_intent_controlled(
        &mut self,
        control: &mut ExecutionControl,
    ) -> Result<(), CheckpointPersistError> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = None;
        self.persist_checkpoint_controlled(control, false, previous).await
    }

    fn persist_checkpoint(&mut self) -> Result<(), String> {
        self.state.updated_at_millis = now_millis();
        self.state.completed_steps = self.state.checkpoint.completed.len();
        self.persist_state().map_err(|error| error.to_string())
    }

    fn persist_checkpoint_or_restore(&mut self, previous: RunCheckpoint) -> Result<(), String> {
        match self.persist_checkpoint() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.state.checkpoint = previous;
                self.state.completed_steps = self.state.checkpoint.completed.len();
                Err(error)
            }
        }
    }

    fn acquire_resume_lock(&mut self) -> Result<(), ResumeError> {
        let path = self.directory.join(RESUME_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|source| ResumeError::Decode(format!("could not open {}: {source}", path.display())))?;
        match file.try_lock() {
            Ok(()) => {
                self.lock_file = Some(file);
                Ok(())
            }
            Err(std::fs::TryLockError::WouldBlock) => Err(ResumeError::Locked(self.state.job_id.clone())),
            Err(std::fs::TryLockError::Error(source)) => Err(ResumeError::Decode(format!(
                "could not lock {}: {source}",
                path.display()
            ))),
        }
    }

    fn write_json(&self, name: &str, value: &impl Serialize, artifact: &'static str) -> Result<(), RunStateError> {
        write_private_json(&self.directory.join(name), value, artifact)
    }
}

impl CheckpointStore for DurableRun {
    fn record_intent(&mut self, step: &PlanStep) -> Result<(), String> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = Some(CheckpointIntent {
            step_id: step.id.clone(),
            tool: step.tool.clone(),
            status: IntentStatus::Dispatched,
        });
        self.persist_checkpoint_or_restore(previous)
    }

    fn record_completion(&mut self, report: &StepReport) -> Result<(), String> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = None;
        self.state.checkpoint.completed.push(report.clone());
        self.persist_checkpoint_or_restore(previous)
    }

    fn record_uncertain(&mut self, step: &PlanStep) -> Result<(), String> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = Some(CheckpointIntent {
            step_id: step.id.clone(),
            tool: step.tool.clone(),
            status: IntentStatus::Uncertain,
        });
        self.persist_checkpoint_or_restore(previous)
    }

    fn clear_intent(&mut self) -> Result<(), String> {
        let previous = self.state.checkpoint.clone();
        self.state.checkpoint.intent = None;
        self.persist_checkpoint_or_restore(previous)
    }
}

impl DurablePostProcessor {
    /// Persist the private report and terminal lifecycle state.
    ///
    /// # Errors
    /// Returns when either artifact cannot be atomically persisted.
    pub fn finish(&mut self, execution: &ExecutionReport) -> Result<(), RunStateError> {
        self.run.finish(execution)
    }

    /// Return the report envelope exposed on stdout or through `--output`.
    #[must_use]
    pub fn report<'a>(&'a self, execution: &'a ExecutionReport) -> DurableExecutionReport<'a> {
        self.run.report(execution)
    }

    /// Persist a terminal stop while detached I/O is in progress.
    ///
    /// # Errors
    /// Returns when the stopped state cannot be atomically persisted.
    pub fn stop(&mut self, reason: ExecutionStopReason) -> Result<(), RunStateError> {
        self.run.stop(reason)
    }

    /// Persist a terminal failure while detached I/O is in progress.
    ///
    /// # Errors
    /// Returns when the failed state cannot be atomically persisted.
    pub fn fail(&mut self, error: &str) -> Result<(), RunStateError> {
        self.run.fail(error)
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), RunStateError> {
    let missing: Vec<_> = path
        .ancestors()
        .take_while(|candidate| !candidate.as_os_str().is_empty() && !candidate.exists())
        .collect();
    for directory in missing.iter().rev() {
        match std::fs::create_dir(directory) {
            Ok(()) => {
                secure_directory(directory)?;
                sync_directory(parent_for_sync(directory))?;
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(io_error(format!("could not create {}", directory.display()), source)),
        }
    }
    let metadata =
        std::fs::metadata(path).map_err(|source| io_error(format!("could not inspect {}", path.display()), source))?;
    if !metadata.is_dir() {
        return Err(io_error(
            format!("{} is not a directory", path.display()),
            std::io::Error::from(std::io::ErrorKind::NotADirectory),
        ));
    }
    secure_directory(path)
}

fn parent_for_sync(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn publish_job_directory(staging: tempfile::TempDir, destination: &Path) -> Result<(), RunStateError> {
    std::fs::rename(staging.path(), destination).map_err(|source| {
        io_error(
            format!("could not publish durable job {}", destination.display()),
            source,
        )
    })?;
    let _published_path = staging.keep();
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), RunStateError> {
    #[cfg(unix)]
    let sync_result = std::fs::File::open(path).and_then(|directory| directory.sync_all());
    #[cfg(windows)]
    let sync_result = {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .and_then(|directory| directory.sync_all())
    };
    #[cfg(any(unix, windows))]
    sync_result.map_err(|source| io_error(format!("could not sync {}", path.display()), source))?;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}

fn secure_directory(path: &Path) -> Result<(), RunStateError> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error(format!("could not secure {}", path.display()), source))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_private_json(path: &Path, value: &impl Serialize, artifact: &'static str) -> Result<(), RunStateError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| RunStateError::Encode { artifact, source })?;
    bytes.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| file.write_all(&bytes).and_then(|()| file.sync_all()), options)
        .map_err(std::io::Error::from)
        .map_err(|source| io_error(format!("could not write {}", path.display()), source))?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|source| io_error(format!("could not secure {}", path.display()), source))?;
    Ok(())
}

fn io_error(operation: String, source: std::io::Error) -> RunStateError {
    RunStateError::Io { operation, source }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::checkpoint::{CheckpointStore, IntentStatus, ResumeError, UncertainPolicy, select_resume};
    use crate::observability::ObservabilitySummary;
    use crate::{PlanStep, StepReport};

    fn plan() -> Plan {
        Plan {
            version: 1,
            steps: vec![PlanStep {
                id: "panels".to_string(),
                tool: "browser_list".to_string(),
                arguments: serde_json::Map::new(),
            }],
        }
    }

    #[cfg(unix)]
    fn empty_report() -> ExecutionReport {
        ExecutionReport {
            version: 1,
            ok: true,
            completed_steps: 0,
            steps: Vec::new(),
            error: None,
            stop_reason: None,
            observability: ObservabilitySummary::default(),
        }
    }

    #[test]
    fn prepared_state_is_a_bounded_lease_before_terminal_report() {
        let home = tempfile::tempdir().expect("temporary home");
        let root = home.path().join(".horizon/browser-jobs");
        let deadline_at_millis = now_millis().saturating_add(30_000);
        let mut run =
            DurableRun::prepare_in(&root, &plan(), Some(30), Some(deadline_at_millis)).expect("prepare durable run");
        let prepared: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("prepared state"))
            .expect("decode prepared state");
        assert_eq!(prepared.version, STATE_VERSION);
        assert_eq!(prepared.status, RunStatus::Prepared);
        assert_eq!(prepared.effective_status_at(deadline_at_millis - 1), RunStatus::Running);
        assert_eq!(prepared.effective_status_at(deadline_at_millis), RunStatus::TimedOut);
        assert_eq!(prepared.completed_steps, 0);
        assert_eq!(prepared.execution_timeout_seconds, Some(30));
        assert_eq!(prepared.deadline_at_millis, Some(deadline_at_millis));
        assert_eq!(prepared.plan_file, PLAN_FILE);
        assert_eq!(
            Plan::from_slice(&std::fs::read(run.directory.join(PLAN_FILE)).expect("saved plan"))
                .expect("decode saved plan"),
            plan()
        );
        let published = std::fs::read_dir(&root)
            .expect("list job root")
            .collect::<Result<Vec<_>, _>>()
            .expect("read job entries");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].path(), run.directory);

        let report = ExecutionReport {
            version: 1,
            ok: true,
            completed_steps: 1,
            steps: vec![StepReport {
                id: "panels".to_string(),
                tool: "browser_list".to_string(),
                ok: true,
                result: Some(json!({"panels": []})),
                error: None,
            }],
            error: None,
            stop_reason: None,
            observability: ObservabilitySummary::default(),
        };
        run.finish(&report).expect("finish durable run");
        let succeeded: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("terminal state"))
            .expect("decode terminal state");
        assert_eq!(succeeded.status, RunStatus::Succeeded);
        assert_eq!(succeeded.completed_steps, 1);
        assert_eq!(succeeded.report_file.as_deref(), Some(REPORT_FILE));
        let saved_report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run.directory.join(REPORT_FILE)).expect("saved report"))
                .expect("decode saved report");
        assert_eq!(saved_report["job_id"], succeeded.job_id);
        assert_eq!(saved_report["ok"], true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let plan_path = run.directory.join(PLAN_FILE);
            let report_path = run.directory.join(REPORT_FILE);
            for (path, mode) in [
                (run.directory.as_path(), 0o700),
                (plan_path.as_path(), 0o600),
                (report_path.as_path(), 0o600),
                (run.state_path.as_path(), 0o600),
            ] {
                assert_eq!(
                    std::fs::metadata(path)
                        .expect("private artifact metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    mode
                );
            }
        }
    }

    #[test]
    fn state_accepts_jobs_created_before_execution_timeouts() {
        let state: RunState = serde_json::from_value(json!({
            "version": 1,
            "job_id": "job-existing",
            "status": "running",
            "created_at_millis": 1,
            "updated_at_millis": 1,
            "runner_pid": 42,
            "plan_file": "plan.json",
            "completed_steps": 0
        }))
        .expect("decode existing durable state");

        assert_eq!(state.execution_timeout_seconds, None);
        assert_eq!(state.deadline_at_millis, None);
        assert_eq!(state.effective_status_at(u64::MAX), RunStatus::Running);

        let prepared_without_deadline: RunState = serde_json::from_value(json!({
            "version": 2,
            "job_id": "job-incomplete",
            "status": "prepared",
            "created_at_millis": 1,
            "updated_at_millis": 1,
            "runner_pid": 42,
            "plan_file": "plan.json",
            "completed_steps": 0
        }))
        .expect("decode incomplete prepared state");
        assert_eq!(prepared_without_deadline.effective_status_at(1), RunStatus::TimedOut);
    }

    #[test]
    fn initialization_failure_is_durable() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(
            root.path(),
            &plan(),
            Some(30),
            Some(now_millis().saturating_add(30_000)),
        )
        .expect("prepare durable run");
        run.fail("adapter unavailable").expect("persist failure");
        let failed: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("failed state"))
            .expect("decode failed state");
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("adapter unavailable"));
        assert!(failed.report_file.is_none());
    }

    #[test]
    fn cancellation_is_a_distinct_terminal_state() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(root.path(), &plan(), Some(30), None).expect("prepare durable run");
        run.stop(ExecutionStopReason::Cancelled).expect("persist cancellation");

        let cancelled: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("cancelled state"))
            .expect("decode cancelled state");
        assert_eq!(cancelled.status, RunStatus::Cancelled);
        assert_eq!(
            cancelled.error.as_deref(),
            Some(ExecutionStopReason::Cancelled.message())
        );
    }

    #[test]
    fn checkpoints_record_intent_then_only_verified_completions() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(root.path(), &two_step_plan(), Some(30), None).expect("prepare");
        let first = &two_step_plan().steps[0];
        run.record_intent(first).expect("intent");
        let dispatched: RunState =
            serde_json::from_slice(&std::fs::read(&run.state_path).expect("state")).expect("decode dispatched");
        assert_eq!(
            dispatched.checkpoint.intent.as_ref().map(|intent| intent.status),
            Some(IntentStatus::Dispatched)
        );
        assert!(dispatched.checkpoint.completed.is_empty());

        let report = StepReport {
            id: first.id.clone(),
            tool: first.tool.clone(),
            ok: true,
            result: Some(json!({"panels": []})),
            error: None,
        };
        run.record_completion(&report).expect("completion");
        let completed: RunState =
            serde_json::from_slice(&std::fs::read(&run.state_path).expect("state")).expect("decode completed");
        assert!(completed.checkpoint.intent.is_none());
        assert_eq!(completed.checkpoint.completed.len(), 1);
        assert_eq!(completed.completed_steps, 1);

        let second = &two_step_plan().steps[1];
        run.record_intent(second).expect("second intent");
        run.record_uncertain(second).expect("uncertain");
        run.stop(ExecutionStopReason::DeadlineExceeded).expect("timeout");
        let timed_out: RunState =
            serde_json::from_slice(&std::fs::read(&run.state_path).expect("state")).expect("decode timed out");
        assert_eq!(
            timed_out.checkpoint.intent.as_ref().map(|intent| intent.status),
            Some(IntentStatus::Uncertain)
        );
        assert_eq!(timed_out.checkpoint.completed.len(), 1);

        let opened = DurableRun::open_in(root.path(), run.job_id()).expect("open");
        assert_eq!(opened.state().checkpoint.completed.len(), 1);
        let selection = select_resume(&two_step_plan(), Some(&timed_out.checkpoint), UncertainPolicy::Skip)
            .expect("skip remaining");
        assert_eq!(selection.start_index, 2);
    }

    #[test]
    fn failed_intent_persist_does_not_keep_in_memory_dispatch() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(root.path(), &two_step_plan(), Some(30), None).expect("prepare");
        std::fs::remove_file(&run.state_path).expect("remove state file");
        std::fs::create_dir(&run.state_path).expect("block state path with a directory");
        run.record_intent(&two_step_plan().steps[0])
            .expect_err("persist must fail");
        assert!(run.state().checkpoint.intent.is_none());
        std::fs::remove_dir(&run.state_path).expect("unblock state path");
    }

    #[test]
    fn resume_lock_is_exclusive_while_the_owner_lives() {
        let root = tempfile::tempdir().expect("temporary job root");
        let run = DurableRun::prepare_in(root.path(), &two_step_plan(), Some(30), None).expect("prepare");
        let mut contender = DurableRun::open_in(root.path(), run.job_id()).expect("open");
        let error = contender.acquire_resume_lock().expect_err("owner holds lock");
        assert!(matches!(error, ResumeError::Locked(_)));
        drop(run);
        contender.acquire_resume_lock().expect("lock released on drop");
    }

    fn two_step_plan() -> Plan {
        Plan {
            version: 1,
            steps: vec![
                PlanStep {
                    id: "list".to_string(),
                    tool: "browser_list".to_string(),
                    arguments: serde_json::Map::new(),
                },
                PlanStep {
                    id: "again".to_string(),
                    tool: "browser_list".to_string(),
                    arguments: serde_json::Map::new(),
                },
            ],
        }
    }

    #[cfg(unix)]
    #[test]
    fn parent_traversal_does_not_resecure_an_existing_directory() {
        let root = tempfile::tempdir().expect("temporary root");
        let existing = root.path().join("existing");
        std::fs::create_dir(&existing).expect("create existing directory");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).expect("set existing permissions");

        ensure_private_directory(&existing.join("missing/../browser-jobs")).expect("create private directory");

        assert_eq!(
            std::fs::metadata(&existing)
                .expect("existing directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn existing_file_is_rejected_without_mutation() {
        let root = tempfile::tempdir().expect("temporary root");
        let file = root.path().join("browser-jobs");
        std::fs::write(&file, b"keep me").expect("write existing file");
        #[cfg(unix)]
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("set existing file permissions");

        let error = ensure_private_directory(&file).expect_err("reject file job root");

        assert!(matches!(
            error,
            RunStateError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotADirectory
        ));
        assert_eq!(std::fs::read(&file).expect("read existing file"), b"keep me");
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&file)
                .expect("existing file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn published_staging_name_can_be_reused_without_cleanup() {
        let root = tempfile::tempdir().expect("temporary root");
        let staging = tempfile::Builder::new()
            .prefix(".preparing-")
            .tempdir_in(root.path())
            .expect("stage job");
        let staging_path = staging.path().to_path_buf();
        let destination = root.path().join("job-published");

        publish_job_directory(staging, &destination).expect("publish job");
        sync_directory(root.path()).expect("sync job root");
        std::fs::create_dir(&staging_path).expect("reuse staging name");
        std::fs::write(staging_path.join("sentinel"), b"keep me").expect("write sentinel");

        assert!(destination.is_dir());
        assert_eq!(
            std::fs::read(staging_path.join("sentinel")).expect("read sentinel"),
            b"keep me"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_report_paths_are_json_safe() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(
            root.path(),
            &plan(),
            Some(30),
            Some(now_millis().saturating_add(30_000)),
        )
        .expect("prepare durable run");
        run.directory = PathBuf::from(OsString::from_vec(b"home-\xff/.horizon/browser-jobs/job".to_vec()));
        run.state_path = run.directory.join(STATE_FILE);

        let encoded = serde_json::to_value(run.report(&empty_report())).expect("encode durable report");

        assert_eq!(encoded["job_dir"], json!(run.directory.display().to_string()));
        assert_eq!(encoded["state_path"], json!(run.state_path.display().to_string()));
    }
}
