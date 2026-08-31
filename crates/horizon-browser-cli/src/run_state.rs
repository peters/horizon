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
    /// Configured MCP execution budget, excluding plan input and durable setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_timeout_seconds: Option<u64>,
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
    /// The configured MCP execution deadline elapsed.
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
pub struct DurableRun {
    directory: PathBuf,
    state_path: PathBuf,
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

impl DurableRun {
    /// Create a private job directory and persist the validated plan plus its
    /// initial running state before any MCP action is attempted.
    ///
    /// # Errors
    /// Returns when the private directory or either initial artifact cannot be
    /// created durably.
    pub fn start(plan: &Plan, execution_timeout_seconds: u64) -> Result<Self, RunStateError> {
        let root = HorizonHome::resolve().root().join("browser-jobs");
        Self::start_in(&root, plan, Some(execution_timeout_seconds))
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

    fn start_in(root: &Path, plan: &Plan, execution_timeout_seconds: Option<u64>) -> Result<Self, RunStateError> {
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
            status: RunStatus::Running,
            created_at_millis,
            execution_timeout_seconds,
            updated_at_millis: created_at_millis,
            runner_pid: std::process::id(),
            plan_file: PLAN_FILE.to_string(),
            report_file: None,
            completed_steps: 0,
            error: None,
        };
        write_private_json(&staging.path().join(PLAN_FILE), plan, "plan")?;
        write_private_json(&staging.path().join(STATE_FILE), &state, "state")?;
        sync_directory(staging.path())?;
        publish_job_directory(staging.path(), &directory, root)?;
        Ok(Self {
            state_path: directory.join(STATE_FILE),
            directory,
            state,
        })
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
        self.state.error = Some(ExecutionStopReason::MESSAGE.to_string());
        self.persist_state()
    }

    fn persist_state(&self) -> Result<(), RunStateError> {
        write_private_json(&self.state_path, &self.state, "state")
    }

    fn write_json(&self, name: &str, value: &impl Serialize, artifact: &'static str) -> Result<(), RunStateError> {
        write_private_json(&self.directory.join(name), value, artifact)
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
    secure_directory(path)
}

fn parent_for_sync(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn publish_job_directory(staging: &Path, destination: &Path, root: &Path) -> Result<(), RunStateError> {
    std::fs::rename(staging, destination).map_err(|source| {
        io_error(
            format!("could not publish durable job {}", destination.display()),
            source,
        )
    })?;
    sync_directory(root)
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
        }
    }

    #[test]
    fn state_is_running_before_execution_and_terminal_after_report() {
        let home = tempfile::tempdir().expect("temporary home");
        let root = home.path().join(".horizon/browser-jobs");
        let mut run = DurableRun::start_in(&root, &plan(), Some(30)).expect("start durable run");
        let running: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("running state"))
            .expect("decode running state");
        assert_eq!(running.status, RunStatus::Running);
        assert_eq!(running.completed_steps, 0);
        assert_eq!(running.execution_timeout_seconds, Some(30));
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
    }

    #[test]
    fn initialization_failure_is_durable() {
        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::start_in(root.path(), &plan(), Some(30)).expect("start durable run");
        run.fail("adapter unavailable").expect("persist failure");
        let failed: RunState = serde_json::from_slice(&std::fs::read(&run.state_path).expect("failed state"))
            .expect("decode failed state");
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("adapter unavailable"));
        assert!(failed.report_file.is_none());
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

    #[cfg(unix)]
    #[test]
    fn non_utf8_report_paths_are_json_safe() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let root = tempfile::tempdir().expect("temporary job root");
        let mut run = DurableRun::start_in(root.path(), &plan(), Some(30)).expect("start durable run");
        run.directory = PathBuf::from(OsString::from_vec(b"home-\xff/.horizon/browser-jobs/job".to_vec()));
        run.state_path = run.directory.join(STATE_FILE);

        let encoded = serde_json::to_value(run.report(&empty_report())).expect("encode durable report");

        assert_eq!(encoded["job_dir"], json!(run.directory.display().to_string()));
        assert_eq!(encoded["state_path"], json!(run.state_path.display().to_string()));
    }
}
