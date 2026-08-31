//! Durable metadata for deterministic browser plan runs.

use std::fs::OpenOptions;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use atomicwrites::{AllowOverwrite, AtomicFile, DisallowOverwrite, replace_atomic};
use horizon_core::HorizonHome;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ExecutionReport, Plan, execution_control::ExecutionStopReason};

const STATE_VERSION: u32 = 1;
const PLAN_FILE: &str = "plan.json";
const REPORT_FILE: &str = "report.json";
const STATE_FILE: &str = "state.json";

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
    /// Configured whole-job budget, including plan input and durable setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_seconds: Option<u64>,
    /// Absolute wall-clock deadline selected when the run started.
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
    /// Private initialization, plan-execution, or MCP shutdown error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Durable plan-run lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The runner may still be active. A later resume slice will distinguish
    /// a live owner from an interrupted process before taking control.
    Running,
    /// Every plan step and MCP shutdown completed successfully.
    Succeeded,
    /// A plan step, MCP connection, or runner operation failed.
    Failed,
    /// The configured whole-job deadline elapsed.
    TimedOut,
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
    activation_path: Option<PathBuf>,
    state: RunState,
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

/// A setup failure that may retain a conservative run for caller-owned failure publication.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct DurablePreparationError {
    #[source]
    source: RunStateError,
    run: Option<Box<DurableRun>>,
}

impl DurablePreparationError {
    fn with_run(run: DurableRun, source: RunStateError) -> Self {
        Self {
            source,
            run: Some(Box::new(run)),
        }
    }

    /// Separate the partial durable run from its setup failure.
    #[must_use]
    pub fn into_parts(self) -> (Option<DurableRun>, RunStateError) {
        (self.run.map(|run| *run), self.source)
    }
}

impl From<RunStateError> for DurablePreparationError {
    fn from(source: RunStateError) -> Self {
        Self { source, run: None }
    }
}

impl DurableRun {
    /// Prepare a private job directory with a conservative timed-out state and
    /// a fully synced running state that only the caller can activate.
    ///
    /// # Errors
    /// Returns when the private directory or an initial artifact cannot be
    /// created durably. Failures after job creation retain the partial run so
    /// the deadline-owning caller can publish `failed`.
    pub fn prepare(
        plan: &Plan,
        execution_timeout_seconds: u64,
        deadline_at_millis: u64,
    ) -> Result<Self, DurablePreparationError> {
        let root = HorizonHome::resolve().root().join("browser-jobs");
        Self::prepare_in(&root, plan, Some(execution_timeout_seconds), Some(deadline_at_millis))
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

    fn prepare_in(
        root: &Path,
        plan: &Plan,
        execution_timeout_seconds: Option<u64>,
        deadline_at_millis: Option<u64>,
    ) -> Result<Self, DurablePreparationError> {
        ensure_private_directory(root)?;
        let job_id = format!("job-{}", Uuid::new_v4());
        let directory = root.join(&job_id);
        let staging = tempfile::Builder::new()
            .prefix(".preparing-")
            .tempdir_in(root)
            .map_err(|source| io_error(format!("could not stage {job_id}"), source))?;
        secure_directory(staging.path())?;
        let created_at_millis = now_millis();
        let state = RunState {
            version: STATE_VERSION,
            job_id,
            status: RunStatus::TimedOut,
            created_at_millis,
            execution_timeout_seconds,
            deadline_at_millis,
            updated_at_millis: created_at_millis,
            runner_pid: std::process::id(),
            plan_file: PLAN_FILE.to_string(),
            report_file: None,
            completed_steps: 0,
            error: Some(ExecutionStopReason::DeadlineExceeded.message().to_string()),
        };
        let mut run = Self {
            directory: staging.path().to_path_buf(),
            state_path: staging.path().join(STATE_FILE),
            activation_path: None,
            state,
        };
        run.write_json(PLAN_FILE, plan, "plan")?;
        run.persist_state()?;
        run.stage_activation()?;
        sync_directory(staging.path())?;
        let activation_name = run
            .activation_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                io_error(
                    "durable job has no staged activation".to_string(),
                    std::io::Error::other("activation staging did not return a path"),
                )
            })?;
        publish_job_directory(staging, &directory)?;
        run.directory = directory;
        run.state_path = run.directory.join(STATE_FILE);
        run.activation_path = Some(run.directory.join(activation_name));
        if let Err(source) = sync_directory(root) {
            return Err(DurablePreparationError::with_run(run, source));
        }
        Ok(run)
    }

    /// Atomically publish the already-synced running state.
    ///
    /// No background preparation worker can perform this transition.
    ///
    /// # Errors
    /// Returns when the staged state cannot replace the conservative state.
    pub fn activate(&mut self) -> Result<(), RunStateError> {
        let activation_path = self.activation_path.as_ref().ok_or_else(|| {
            io_error(
                "durable job has no pending activation".to_string(),
                std::io::Error::other("activation already consumed"),
            )
        })?;
        // `replace_atomic` fsyncs parent directories on Unix and requests a
        // write-through replacement on Windows.
        replace_atomic(activation_path, &self.state_path)
            .map_err(|source| io_error(format!("could not activate {}", self.state_path.display()), source))?;
        self.activation_path = None;
        self.state.status = RunStatus::Running;
        self.state.error = None;
        Ok(())
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
    pub fn stop(&mut self, _reason: ExecutionStopReason) -> Result<(), RunStateError> {
        self.state.status = RunStatus::TimedOut;
        self.state.updated_at_millis = now_millis();
        self.state.error = Some(ExecutionStopReason::DeadlineExceeded.message().to_string());
        self.persist_state()
    }

    fn persist_state(&self) -> Result<(), RunStateError> {
        write_private_json(&self.state_path, &self.state, "state")
    }

    fn stage_activation(&mut self) -> Result<(), RunStateError> {
        let mut running = self.state.clone();
        running.status = RunStatus::Running;
        running.updated_at_millis = now_millis();
        running.error = None;
        let path = self
            .directory
            .join(format!(".state-activation-{}.json", Uuid::new_v4()));
        self.activation_path = Some(path.clone());
        write_private_staged_json(&path, &running, "state")?;
        Ok(())
    }

    fn write_json(&self, name: &str, value: &impl Serialize, artifact: &'static str) -> Result<(), RunStateError> {
        write_private_json(&self.directory.join(name), value, artifact)
    }
}

impl Drop for DurableRun {
    fn drop(&mut self) {
        if let Some(path) = self.activation_path.take() {
            let _ = std::fs::remove_file(path);
        }
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

fn write_private_staged_json(path: &Path, value: &impl Serialize, artifact: &'static str) -> Result<(), RunStateError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| RunStateError::Encode { artifact, source })?;
    bytes.push(b'\n');
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    // The no-overwrite commit persists both the file and its directory entry
    // before preparation can return it to the caller for activation.
    AtomicFile::new(path, DisallowOverwrite)
        .write_with_options(|file| file.write_all(&bytes), options)
        .map_err(std::io::Error::from)
        .map_err(|source| io_error(format!("could not stage {}", path.display()), source))
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
    use std::sync::mpsc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::{
        PlanStep, StepReport,
        execution_control::{ExecutionControl, JobDeadline},
    };

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
        }
    }

    #[test]
    fn caller_activates_a_conservatively_prepared_run_before_execution() {
        let root = tempfile::tempdir().expect("temporary job root");
        let deadline_at_millis = now_millis().saturating_add(30_000);
        let mut run = DurableRun::prepare_in(root.path(), &plan(), Some(30), Some(deadline_at_millis))
            .expect("prepare durable run");
        let prepared: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("prepared state"))
            .expect("decode prepared state");
        assert_eq!(prepared.status, RunStatus::TimedOut);
        assert_eq!(
            prepared.error.as_deref(),
            Some(ExecutionStopReason::DeadlineExceeded.message())
        );

        run.activate().expect("activate durable run");
        let running: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("running state"))
            .expect("decode running state");
        assert_eq!(running.status, RunStatus::Running);
        assert_eq!(running.completed_steps, 0);
        assert_eq!(running.execution_timeout_seconds, Some(30));
        assert_eq!(running.deadline_at_millis, Some(deadline_at_millis));
        assert_eq!(running.error, None);
        assert_eq!(running.plan_file, PLAN_FILE);
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
    fn abandoned_preparation_cannot_publish_running_state() {
        let root = tempfile::tempdir().expect("temporary job root");
        let state_path = {
            let run = DurableRun::prepare_in(root.path(), &plan(), Some(1), Some(now_millis().saturating_add(1_000)))
                .expect("prepare durable run");
            run.state_path.clone()
        };

        let state: RunState = serde_json::from_slice(&std::fs::read(&state_path).expect("prepared state"))
            .expect("decode prepared state");
        assert_eq!(state.status, RunStatus::TimedOut);
        let directory = state_path.parent().expect("job directory");
        let names = std::fs::read_dir(directory)
            .expect("job artifacts")
            .map(|entry| entry.expect("job artifact").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert!(directory.join(PLAN_FILE).exists());
        assert!(directory.join(STATE_FILE).exists());
    }

    #[tokio::test]
    async fn deadline_abandons_active_preparation_without_publishing_running() {
        let root = tempfile::tempdir().expect("temporary job root");
        let worker_root = root.path().to_path_buf();
        let worker_plan = plan();
        let (prepared_sender, prepared_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        let (finished_sender, finished_receiver) = mpsc::sync_channel(0);
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            let run = DurableRun::prepare_in(
                &worker_root,
                &worker_plan,
                Some(1),
                Some(now_millis().saturating_add(1_000)),
            )
            .expect("prepare durable run");
            prepared_sender
                .send(run.state_path.clone())
                .expect("publish prepared state path");
            release_receiver.recv().expect("release preparation worker");
            let _ = result_sender.send(run);
            finished_sender.send(()).expect("publish worker completion");
        });

        let state_path = prepared_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("prepared state path");
        let directory = state_path.parent().expect("job directory");
        assert_eq!(activation_files(directory).len(), 1);

        let mut control = ExecutionControl::until(JobDeadline::after(Duration::ZERO));
        assert!(matches!(
            control.wait(result_receiver).await,
            Err(ExecutionStopReason::DeadlineExceeded)
        ));
        release_sender.send(()).expect("release preparation worker");
        finished_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("late preparation cleanup");
        worker.join().expect("preparation worker");

        let state: RunState = serde_json::from_slice(&std::fs::read(&state_path).expect("timed-out state"))
            .expect("decode timed-out state");
        assert_eq!(state.status, RunStatus::TimedOut);
        assert!(activation_files(directory).is_empty());
    }

    fn activation_files(directory: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(directory)
            .expect("job artifacts")
            .map(|entry| entry.expect("job artifact").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".state-activation-"))
            })
            .collect()
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
    }

    #[test]
    fn initialization_failure_is_durable() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::prepare_in(root.path(), &plan(), Some(30), None).expect("prepare durable run");
        run.activate().expect("activate durable run");
        run.fail("adapter unavailable").expect("persist failure");
        let failed: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("failed state"))
            .expect("decode failed state");
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("adapter unavailable"));
        assert!(failed.report_file.is_none());
    }

    #[test]
    fn known_preparation_failure_retains_a_run_for_failed_publication() {
        let root = tempfile::tempdir().expect("temporary job root");
        let run = DurableRun::prepare_in(root.path(), &plan(), Some(30), None).expect("prepare durable run");
        let failure = DurablePreparationError::with_run(
            run,
            io_error(
                "could not finish preparation".to_string(),
                std::io::Error::other("synthetic setup failure"),
            ),
        );
        let (run, source) = failure.into_parts();
        let mut run = run.expect("partial durable run");
        run.fail(&source.to_string()).expect("publish failed state");

        let failed: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("failed state"))
            .expect("decode failed state");
        assert_eq!(failed.status, RunStatus::Failed);
        assert!(
            failed
                .error
                .as_deref()
                .is_some_and(|error| error.contains("synthetic setup failure"))
        );
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
        let mut run = DurableRun::prepare_in(root.path(), &plan(), Some(30), None).expect("prepare durable run");
        run.activate().expect("activate durable run");
        run.directory = PathBuf::from(OsString::from_vec(b"home-\xff/.horizon/browser-jobs/job".to_vec()));
        run.state_path = run.directory.join(STATE_FILE);

        let encoded = serde_json::to_value(run.report(&empty_report())).expect("encode durable report");

        assert_eq!(encoded["job_dir"], json!(run.directory.display().to_string()));
        assert_eq!(encoded["state_path"], json!(run.state_path.display().to_string()));
    }
}
