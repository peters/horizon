use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_browser_control::manifest::provider_usage::{ProviderUsageSummary, UsageQueue};
use horizon_browser_control::manifest::{self, BrowserManifest};
use serde_json::{Value, json};

// Process startup and manifest persistence must leave time for the blocking action.
const DEADLINE_TEST_TIMEOUT_SECONDS: u64 = 6;
// A starved job can reach its deadline before it queues the blocking action; it then
// runs again with twice the deadline, up to this one.
const MAX_DEADLINE_TEST_TIMEOUT_SECONDS: u64 = 8 * DEADLINE_TEST_TIMEOUT_SECONDS;
// Polls for job progress return as soon as the job reaches the awaited state; this bound
// only turns a hung job into a failure, so it must exceed what a healthy job can take.
// The first launch of a freshly linked binary can take seconds on Windows while it is
// scanned, and after a deadline every durable write may retry a blocked replace for up to
// two seconds before the job exits.
const JOB_PROGRESS_GUARD: Duration = Duration::from_mins(1);
const STDIN_EXECUTION_TIMEOUT: Duration = Duration::from_secs(1);
const DEADLINE_ROUNDING_SLACK_MILLIS: u128 = 1;

#[test]
fn drop_files_plan_dispatches_the_public_action_with_synthetic_bytes() {
    use horizon_browser::{AgentActionResult, BrowserControlAction, BrowserControlValue, BrowserTarget};

    let root = tempfile::tempdir().unwrap();
    let panel_id = "synthetic-drop-panel";
    let file = root.path().join("Overview.pdf");
    std::fs::write(&file, b"synthetic drop bytes").unwrap();
    let plan = root.path().join("drop-plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "version":1,
            "steps":[{"id":"drop","tool":"browser_act","arguments":{
                "panel_id":panel_id,"action":"drop_files","selector":"#drop-zone","files":[file]
            }}]
        }))
        .unwrap(),
    )
    .unwrap();
    let manifest_path = manifest::manifest_path_for_root(&root.path().join(".horizon"), panel_id);
    manifest::write_at(
        &manifest_path,
        &BrowserManifest {
            panel_local_id: panel_id.into(),
            ..BrowserManifest::default()
        },
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().unwrap()])
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ROOT", root.path().join(".horizon"))
        .env_remove("HORIZON")
        .env("HORIZON_WORK_ROOT", root.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_manifest_action(&mut child, &manifest_path);
    let queued: BrowserManifest = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(queued.actions.len(), 1);
    let action = &queued.actions[0];
    let BrowserControlAction::DropFiles { target, paths, .. } = &action.action else {
        panic!("plan did not dispatch DropFiles");
    };
    assert_eq!(
        target,
        &BrowserTarget::Selector {
            selector: "#drop-zone".into()
        }
    );
    assert_eq!(paths.len(), 1);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"synthetic drop bytes");
    let result = AgentActionResult::completed(action.action_id.clone(), BrowserControlValue::Accepted);
    answer_action(root.path(), panel_id, &result);
    wait_for_exit(&mut child, "synthetic drop plan");
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}; report: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][0]["result"]["completed"], true);
    assert_eq!(report["steps"][0]["result"]["action_id"], action.action_id);
}

#[test]
fn run_writes_the_same_structured_report_to_stdout_or_a_private_file() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}"#,
    )
    .expect("write plan");

    let stdout = run_command(root.path(), ["run", plan.to_str().expect("UTF-8 path")]);
    assert!(
        stdout.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stdout.stderr)
    );
    let stdout_report: Value = serde_json::from_slice(&stdout.stdout).expect("stdout report");
    assert_eq!(stdout_report["ok"], true);
    assert_eq!(stdout_report["observability"]["audit"]["observed"], false);
    assert_eq!(stdout_report["observability"]["network"]["observed"], false);
    assert_eq!(stdout_report["steps"][0]["tool"], "browser_list");
    assert_eq!(stdout_report["steps"][0]["result"]["panels"], json!([]));
    let job_dir = std::path::PathBuf::from(stdout_report["job_dir"].as_str().expect("job directory"));
    assert!(job_dir.starts_with(root.path().join(".horizon/browser-jobs")));
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("job state"))
        .expect("decode job state");
    assert_eq!(state["job_id"], stdout_report["job_id"]);
    assert_eq!(state["status"], "succeeded");
    assert_eq!(state["execution_timeout_seconds"], 1800);
    assert!(state["deadline_at_millis"].as_u64().is_some());
    assert_eq!(state["completed_steps"], 1);
    assert_eq!(state["report_file"], "report.json");
    assert_checkpoint_artifact(&job_dir, &state, &stdout_report["steps"][0]);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(job_dir.join("report.json")).expect("durable report"))
            .expect("decode durable report"),
        stdout_report
    );

    let report = root.path().join("report.json");
    let file = run_command(
        root.path(),
        [
            "run",
            plan.to_str().expect("UTF-8 path"),
            "--output",
            report.to_str().expect("UTF-8 path"),
        ],
    );
    assert!(
        file.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&file.stderr)
    );
    assert!(file.stdout.is_empty());
    let file_report: Value =
        serde_json::from_slice(&std::fs::read(&report).expect("read report")).expect("file report");
    assert_ne!(file_report["job_id"], stdout_report["job_id"]);
    assert_eq!(file_report["ok"], stdout_report["ok"]);
    assert_eq!(file_report["completed_steps"], stdout_report["completed_steps"]);
    assert_eq!(file_report["steps"], stdout_report["steps"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(report).expect("report metadata").permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(job_dir.join("state.json"))
                .expect("state metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    std::fs::write(
        &plan,
        br##"{"version":1,"steps":[{"id":"fill","tool":"browser_act","arguments":{"panel_id":"missing","action":"fill","selector":"#secret","value":"do-not-echo"}},{"id":"never","tool":"browser_list"}]}"##,
    )
    .expect("write failing plan");
    let failed = run_command(root.path(), ["run", plan.to_str().expect("UTF-8 path")]);
    assert!(!failed.status.success());
    let failure_report: Value = serde_json::from_slice(&failed.stdout).expect("failure report");
    assert_eq!(failure_report["completed_steps"], 1);
    assert_eq!(failure_report["steps"].as_array().map(Vec::len), Some(1));
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("do-not-echo"));
    let failed_state: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(failure_report["job_dir"].as_str().expect("failed job directory")).join("state.json"),
        )
        .expect("failed job state"),
    )
    .expect("decode failed job state");
    assert_eq!(failed_state["status"], "failed");
    assert_eq!(failed_state["completed_steps"], 1);
    assert_eq!(failed_state["checkpoint"]["intent"]["status"], "uncertain");
    assert_eq!(failed_state["checkpoint"]["intent"]["step_id"], "fill");
    assert!(failed_state["checkpoint"].get("completed").is_none());
}

#[test]
fn run_projects_json_from_a_prior_step() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"variables":{"note":"list-only"},"steps":[{"id":"panels","tool":"browser_list"}],"project":{"format":"json","from":{"$ref":"panels#/panels"}}}"#,
    )
    .expect("write plan");

    let output = run_command(root.path(), ["run", plan.to_str().expect("UTF-8 path")]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["projection"]["format"], "json");
    assert_eq!(report["projection"]["file"], "projection.json");
    assert_eq!(report["projection"]["rows"], 0);
    let job_dir = std::path::PathBuf::from(report["job_dir"].as_str().expect("job directory"));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(job_dir.join("projection.json")).expect("projection"))
            .expect("decode projection"),
        json!([])
    );
}

#[test]
fn run_fails_when_a_requested_projection_cannot_be_produced() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"panels","tool":"browser_list"}],"project":{"format":"json","from":{"$ref":"panels#/missing"}}}"#,
    )
    .expect("write plan");

    let output = run_command(root.path(), ["run", plan.to_str().expect("UTF-8 path")]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout report");
    assert_eq!(report["ok"], false);
    assert!(report.get("projection").is_none());
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|error| error.contains("did not match")),
        "error: {}",
        report["error"]
    );
    let job_dir = std::path::PathBuf::from(report["job_dir"].as_str().expect("job directory"));
    assert!(!job_dir.join("projection.json").is_file());
}

fn assert_checkpoint_artifact(job_dir: &std::path::Path, state: &Value, expected: &Value) {
    assert!(state["checkpoint"]["completed"][0].get("result").is_none());
    let checkpoint_report = job_dir.join(
        state["checkpoint"]["completed"][0]["report_file"]
            .as_str()
            .expect("checkpoint result path"),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&checkpoint_report).expect("checkpoint result"))
            .expect("decode checkpoint result"),
        *expected
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            std::fs::metadata(job_dir.join("checkpoints"))
                .expect("checkpoint directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(checkpoint_report)
                .expect("checkpoint result metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn run_publishes_a_complete_relative_job_when_home_is_unset() {
    let current_dir = tempfile::tempdir().expect("isolated working directory");
    let plan = current_dir.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}"#,
    )
    .expect("write plan");

    let output = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().expect("UTF-8 path")])
        .current_dir(current_dir.path())
        .env_remove("HOME")
        // Windows falls back to the user profile; this covers having neither.
        .env_remove("USERPROFILE")
        .env_remove("HORIZON")
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .output()
        .expect("run horizon-browser without HOME");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout report");
    let relative_job_dir = std::path::PathBuf::from(report["job_dir"].as_str().expect("job directory"));
    assert!(relative_job_dir.starts_with(".horizon/browser-jobs"));
    let job_dir = current_dir.path().join(&relative_job_dir);
    let entries = std::fs::read_dir(current_dir.path().join(".horizon/browser-jobs"))
        .expect("job root")
        .collect::<Result<Vec<_>, _>>()
        .expect("job entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path(), job_dir);
    assert!(job_dir.join("plan.json").is_file());
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("job state"))
        .expect("decode job state");
    assert_eq!(state["version"], 4);
    assert_eq!(state["status"], "succeeded");
    assert!(state["deadline_at_millis"].as_u64().is_some());
    assert_eq!(state["report_file"], "report.json");
}

#[test]
fn run_persists_preflight_failure_before_any_browser_action() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan = root.path().join("unknown-tool.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"unknown","tool":"browser_missing"}]}"#,
    )
    .expect("write plan");

    let output = run_command(root.path(), ["run", plan.to_str().expect("UTF-8 path")]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let jobs = root.path().join(".horizon").join("browser-jobs");
    let entries = std::fs::read_dir(&jobs)
        .expect("job directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("job entries");
    assert_eq!(entries.len(), 1);
    let job_dir = entries[0].path();
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("failed state"))
        .expect("decode failed state");
    assert_eq!(state["status"], "failed");
    assert_eq!(state["completed_steps"], 0);
    assert!(
        state["error"]
            .as_str()
            .is_some_and(|error| error.contains("unavailable MCP tool"))
    );
    assert!(!job_dir.join("report.json").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(state["job_id"].as_str().expect("job id")));
    assert!(stderr.contains(job_dir.join("state.json").to_string_lossy().as_ref()));
}

#[test]
fn run_deadline_persists_a_partial_report_and_stable_exit_code() {
    let root = tempfile::tempdir().expect("isolated root");
    let (plan, manifest_path) = write_blocking_plan(root.path());
    let DeadlineRun {
        output,
        timeout_seconds,
    } = run_deadline_after_action(root.path(), &plan, &manifest_path, None);

    assert_eq!(output.status.code(), Some(124));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("deadline report");
    assert_eq!(report["completed_steps"], 1);
    assert_eq!(report["stop_reason"], "deadline_exceeded");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|error| error.contains("in-flight browser action may still complete"))
    );
    let job_dir = std::path::Path::new(report["job_dir"].as_str().expect("job directory"));
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("deadline state"))
        .expect("decode deadline state");
    assert_eq!(state["status"], "timed_out");
    assert_eq!(state["execution_timeout_seconds"], timeout_seconds);
    assert!(state["deadline_at_millis"].as_u64().is_some());
    assert_eq!(state["completed_steps"], 1);
    assert_eq!(state["report_file"], "report.json");
    let failed_root = tempfile::tempdir().expect("isolated failed-output root");
    let (failed_plan, failed_manifest_path) = write_blocking_plan(failed_root.path());
    let failed_output = run_deadline_after_action(
        failed_root.path(),
        &failed_plan,
        &failed_manifest_path,
        Some(failed_root.path()),
    )
    .output;
    assert_eq!(failed_output.status.code(), Some(124));
    assert!(String::from_utf8_lossy(&failed_output.stderr).contains("could not open report"));
}

#[test]
fn resume_refuses_an_uncertain_in_flight_step() {
    let root = tempfile::tempdir().expect("isolated root");
    let (plan, manifest_path) = write_blocking_plan(root.path());
    let output = run_deadline_after_action(root.path(), &plan, &manifest_path, None).output;
    assert_eq!(output.status.code(), Some(124));
    let report: Value = serde_json::from_slice(&output.stdout).expect("deadline report");
    let job_id = report["job_id"].as_str().expect("job id");
    let job_dir = std::path::Path::new(report["job_dir"].as_str().expect("job directory"));
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("deadline state"))
        .expect("decode deadline state");
    assert_eq!(state["checkpoint"]["completed"].as_array().map(Vec::len), Some(1));
    assert_eq!(state["checkpoint"]["intent"]["status"], "uncertain");
    assert_eq!(state["checkpoint"]["intent"]["step_id"], "snapshot");

    let refused = run_command(root.path(), ["resume", job_id]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("uncertain step `snapshot`"),
        "stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let skipped = run_command(root.path(), ["resume", job_id, "--on-uncertain", "skip"]);
    assert_eq!(skipped.status.code(), Some(1));
    assert!(
        skipped.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    let skipped_report: Value = serde_json::from_slice(&skipped.stdout).expect("skipped final-step report");
    assert_eq!(skipped_report["ok"], false);
    assert_eq!(skipped_report["completed_steps"], 1);
    assert_eq!(
        skipped_report["steps"]
            .as_array()
            .expect("step reports")
            .iter()
            .map(|step| step["id"].as_str())
            .collect::<Vec<_>>(),
        [Some("list")]
    );
    assert_eq!(
        skipped_report["error"],
        "plan remains incomplete because resume explicitly skipped uncertain steps: snapshot"
    );
    let skipped_state: Value =
        serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("skipped final-step state"))
            .expect("decode skipped final-step state");
    assert_eq!(skipped_state["status"], "failed");
    assert_eq!(skipped_state["report_file"], "report.json");
    assert_eq!(skipped_state["checkpoint"]["skipped"], json!(["snapshot"]));
    assert!(skipped_state["checkpoint"].get("intent").is_none());
}

#[test]
fn resume_refuses_a_dead_standalone_host() {
    let root = tempfile::tempdir().expect("isolated root");
    let (plan, manifest_path) = write_blocking_plan(root.path());
    let output = run_deadline_after_action(root.path(), &plan, &manifest_path, None).output;
    assert_eq!(output.status.code(), Some(124));
    let report: Value = serde_json::from_slice(&output.stdout).expect("deadline report");
    let job_id = report["job_id"].as_str().expect("job id");
    let job_dir = std::path::Path::new(report["job_dir"].as_str().expect("job directory"));
    std::fs::write(
        job_dir.join("standalone.json"),
        br#"{"panel_id":"standalone-1-missing","host_pid":1}"#,
    )
    .expect("record dead standalone host");

    let refused = run_command(root.path(), ["resume", job_id, "--on-uncertain", "skip"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("gone and cannot be reconnected"),
        "stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn resume_prunes_dead_hosts_without_a_sidecar() {
    let root = tempfile::tempdir().expect("isolated root");
    let (plan, manifest_path) = write_blocking_plan(root.path());
    let output = run_deadline_after_action(root.path(), &plan, &manifest_path, None).output;
    assert_eq!(output.status.code(), Some(124));
    let report: Value = serde_json::from_slice(&output.stdout).expect("deadline report");
    let job_id = report["job_id"].as_str().expect("job id");

    let horizon = root.path().join(".horizon");
    let panel_id = "standalone-9-dead";
    let dead_manifest = manifest::manifest_path_for_root(&horizon, panel_id);
    std::fs::create_dir_all(dead_manifest.parent().expect("browsers directory")).expect("browsers dir");
    std::fs::write(
        dead_manifest.with_extension("lease.json"),
        br#"{"panel_id":"standalone-9-dead","host_pid":0}"#,
    )
    .expect("record dead lease");
    manifest::write_at(
        &dead_manifest,
        &BrowserManifest {
            panel_local_id: panel_id.to_string(),
            ..BrowserManifest::default()
        },
    )
    .expect("record dead manifest");

    let _ = run_command(root.path(), ["resume", job_id, "--on-uncertain", "skip"]);
    assert!(
        !dead_manifest.with_extension("lease.json").is_file(),
        "resume must prune dead standalone hosts even when the job has no sidecar"
    );
    assert!(manifest::read_at(&dead_manifest).is_none());
}

#[test]
fn resume_skip_runs_later_steps_without_replaying_or_succeeding() {
    let root = tempfile::tempdir().expect("isolated root");
    let (plan, manifest_path) = write_blocking_plan_with_followup(root.path());
    let output = run_deadline_after_action(root.path(), &plan, &manifest_path, None).output;
    assert_eq!(output.status.code(), Some(124));
    let report: Value = serde_json::from_slice(&output.stdout).expect("deadline report");
    let job_id = report["job_id"].as_str().expect("job id");
    let job_dir = std::path::Path::new(report["job_dir"].as_str().expect("job directory"));
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("deadline state"))
        .expect("decode deadline state");
    assert_eq!(state["checkpoint"]["completed"][0]["id"], "list");
    assert_eq!(state["checkpoint"]["intent"]["status"], "uncertain");
    assert_eq!(state["checkpoint"]["intent"]["step_id"], "snapshot");

    let skipped = run_command(root.path(), ["resume", job_id, "--on-uncertain", "skip"]);
    assert_eq!(skipped.status.code(), Some(1));
    assert!(
        skipped.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    let resume_report: Value = serde_json::from_slice(&skipped.stdout).expect("skip resume report");
    assert_eq!(resume_report["ok"], false);
    assert_eq!(resume_report["completed_steps"], 2);
    assert_eq!(
        resume_report["error"],
        "plan remains incomplete because resume explicitly skipped uncertain steps: snapshot"
    );
    assert_eq!(
        resume_report["steps"]
            .as_array()
            .expect("step reports")
            .iter()
            .map(|step| step["id"].as_str())
            .collect::<Vec<_>>(),
        [Some("list"), Some("followup")]
    );

    let resumed: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("resumed state"))
        .expect("decode resumed state");
    assert_eq!(resumed["status"], "failed");
    assert_eq!(resumed["checkpoint"]["skipped"], json!(["snapshot"]));
    assert_eq!(
        resumed["checkpoint"]["completed"]
            .as_array()
            .expect("completed reports")
            .iter()
            .map(|step| step["id"].as_str())
            .collect::<Vec<_>>(),
        [Some("list"), Some("followup")]
    );
    assert!(resumed["checkpoint"].get("intent").is_none());
}

#[test]
fn run_timeout_starts_after_stdin_plan_validation() {
    let root = tempfile::tempdir().expect("isolated root");
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", "-", "--timeout", &STDIN_EXECUTION_TIMEOUT.as_secs().to_string()])
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin browser job");
    let mut stdin = child.stdin.take().expect("open child stdin");
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        child.try_wait().expect("poll stdin browser job").is_none(),
        "action timeout elapsed while plan stdin was still open"
    );
    stdin
        .write_all(br#"{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}"#)
        .expect("write delayed plan");
    let before_eof = SystemTime::now().duration_since(UNIX_EPOCH).expect("EOF clock");
    drop(stdin);
    wait_for_exit(&mut child, "delayed stdin browser job");
    let output = child.wait_with_output().expect("collect delayed-plan browser job");
    let after_exit = SystemTime::now().duration_since(UNIX_EPOCH).expect("exit clock");
    assert!(
        after_exit >= before_eof,
        "wall clock moved backwards: before EOF {before_eof:?}, after exit {after_exit:?}"
    );
    assert_stdin_deadline_result(root.path(), &output, before_eof, after_exit);
}

fn assert_stdin_deadline_result(
    root: &std::path::Path,
    output: &std::process::Output,
    before_eof: Duration,
    after_exit: Duration,
) {
    assert!(
        matches!(output.status.code(), Some(0 | 124)),
        "exit: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let jobs = std::fs::read_dir(root.join(".horizon/browser-jobs"))
        .expect("delayed-plan jobs")
        .collect::<Result<Vec<_>, _>>()
        .expect("job entries");
    assert_eq!(jobs.len(), 1);
    assert!(
        jobs[0].file_type().expect("job entry type").is_dir(),
        "single job entry must be a directory"
    );
    let job_dir = jobs[0].path();
    let state: Value = serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).expect("job state"))
        .expect("decode job state");
    assert_eq!(state["version"], 4);
    assert_eq!(state["execution_timeout_seconds"], STDIN_EXECUTION_TIMEOUT.as_secs());
    assert!(state["completed_steps"].as_u64().is_some_and(|count| count <= 1));
    let deadline = u128::from(state["deadline_at_millis"].as_u64().expect("saved deadline"));
    let created = u128::from(state["created_at_millis"].as_u64().expect("saved creation time"));
    assert!(
        created + DEADLINE_ROUNDING_SLACK_MILLIS >= before_eof.as_millis() && created <= after_exit.as_millis(),
        "saved creation {created} is outside before EOF {before_eof:?} and after exit {after_exit:?}"
    );
    // Check timer admission, not filesystem speed; allow only millisecond rounding slack.
    let timeout_millis = STDIN_EXECUTION_TIMEOUT.as_millis();
    assert!(
        deadline + DEADLINE_ROUNDING_SLACK_MILLIS >= before_eof.as_millis() + timeout_millis,
        "saved deadline {deadline} predates EOF {before_eof:?} plus timeout {STDIN_EXECUTION_TIMEOUT:?}"
    );
    assert!(
        deadline <= created + timeout_millis + DEADLINE_ROUNDING_SLACK_MILLIS,
        "saved deadline {deadline} exceeds creation {created} plus timeout {STDIN_EXECUTION_TIMEOUT:?}"
    );
    assert_eq!(
        state["job_id"].as_str(),
        job_dir.file_name().and_then(|name| name.to_str())
    );
    let success = output.status.success();
    assert_eq!(state["status"], if success { "succeeded" } else { "timed_out" });
    if success {
        assert_eq!(state["completed_steps"], 1);
    } else {
        assert!(deadline <= after_exit.as_millis());
        assert!(
            state["error"]
                .as_str()
                .is_some_and(|error| error.contains("job deadline exceeded"))
        );
    }
    if state["report_file"].is_null() {
        assert!(!success);
        assert_eq!(state["completed_steps"], 0);
        assert!(output.stdout.is_empty());
        assert!(!job_dir.join("report.json").exists());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("job deadline exceeded"));
        assert!(stderr.contains(state["job_id"].as_str().expect("saved job id")));
    } else {
        assert_eq!(state["report_file"], "report.json");
        assert!(output.stderr.is_empty());
        let report: Value = serde_json::from_slice(&output.stdout).expect("delayed-plan report");
        let saved: Value = serde_json::from_slice(&std::fs::read(job_dir.join("report.json")).expect("saved report"))
            .expect("decode saved report");
        assert_eq!(saved, report);
        assert_eq!(report["job_id"], state["job_id"]);
        assert_eq!(report["ok"], success);
        assert_eq!(report["completed_steps"], state["completed_steps"]);
        assert_eq!(
            report["steps"].as_array().map(Vec::len),
            state["completed_steps"]
                .as_u64()
                .and_then(|count| usize::try_from(count).ok())
        );
        assert_eq!(
            report["stop_reason"],
            if success {
                Value::Null
            } else {
                json!("deadline_exceeded")
            }
        );
    }
}

#[cfg(unix)]
#[test]
fn interrupt_persists_cancelled_partial_report_and_exit_130() {
    let mut attempts = 0;
    let (_root, output) = loop {
        attempts += 1;
        let root = tempfile::tempdir().expect("isolated root");
        let (plan, manifest_path) = write_blocking_plan(root.path());
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
            .args(["run", plan.to_str().expect("UTF-8 path"), "--timeout", "30"])
            .env("HOME", root.path())
            .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cancellable browser job");
        wait_for_manifest_action(&mut child, &manifest_path);
        send_interrupt(child.id());
        wait_for_exit(&mut child, "cancelled browser job");
        let output = child.wait_with_output().expect("collect cancelled browser job");
        // After an interrupt the job gives its report one second, then exits 130 without
        // it. A starved job can miss that grace; such a run tests nothing about the report.
        if output.status.code() == Some(130) && output.stdout.is_empty() && attempts < 3 {
            continue;
        }
        break (root, output);
    };

    assert_eq!(output.status.code(), Some(130));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("cancelled report");
    assert_eq!(report["completed_steps"], 1);
    assert_eq!(report["stop_reason"], "cancelled");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|error| error.contains("in-flight browser action may still complete"))
    );
    let state: Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(report["state_path"].as_str().expect("state path")))
            .expect("cancelled state"),
    )
    .expect("decode cancelled state");
    assert_eq!(state["status"], "cancelled");
    assert_eq!(state["completed_steps"], 1);
    assert_eq!(state["report_file"], "report.json");
}

#[cfg(unix)]
#[test]
fn interrupt_bounds_blocked_plan_input_before_durable_setup() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan_pipe = root.path().join("plan.pipe");
    let writer_ready = root.path().join("writer-ready");
    assert!(
        Command::new("mkfifo")
            .arg(&plan_pipe)
            .status()
            .expect("create plan FIFO")
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan_pipe.to_str().expect("UTF-8 FIFO path"), "--timeout", "30"])
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn blocked-input browser job");
    let mut writer = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec 3>\"$1\"\n: >\"$2\"\nexec sleep 30")
        .arg("fifo-writer")
        .arg(&plan_pipe)
        .arg(&writer_ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn plan FIFO writer");
    let readiness_deadline = Instant::now() + JOB_PROGRESS_GUARD;
    while !writer_ready.exists() {
        let failure = if child.try_wait().expect("poll blocked-input browser job").is_some() {
            Some("browser job exited before blocking on plan input")
        } else if writer.try_wait().expect("poll plan FIFO writer").is_some() {
            Some("plan FIFO writer exited before its reader was ready")
        } else if Instant::now() >= readiness_deadline {
            Some("browser job did not begin reading its plan before the readiness deadline")
        } else {
            None
        };
        if let Some(message) = failure {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.kill();
            let _ = writer.wait();
            panic!("{message}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    send_interrupt(child.id());
    wait_for_exit(&mut child, "cancelled blocked-input browser job");
    let output = child
        .wait_with_output()
        .expect("collect cancelled blocked-input browser job");
    let _ = writer.kill();
    writer.wait().expect("reap plan FIFO writer");

    assert_eq!(output.status.code(), Some(130));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("job cancelled by interrupt"));
    assert!(!stderr.contains("in-flight browser action"));
    assert!(!root.path().join(".horizon/browser-jobs").exists());
}

#[cfg(unix)]
#[test]
fn interrupt_terminates_blocked_report_finalization() {
    let root = tempfile::tempdir().expect("isolated root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}"#,
    )
    .expect("write plan");
    let report_pipe = root.path().join("report.pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&report_pipe)
            .status()
            .expect("create report FIFO")
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args([
            "run",
            plan.to_str().expect("UTF-8 plan"),
            "--output",
            report_pipe.to_str().expect("UTF-8 FIFO"),
        ])
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn browser job with blocked report output");
    let jobs = root.path().join(".horizon/browser-jobs");
    let state_path = wait_for_job_status(&mut child, &jobs, "succeeded");

    send_interrupt(child.id());
    wait_for_exit(&mut child, "blocked report finalization");
    let output = child.wait_with_output().expect("collect blocked finalization output");

    assert_eq!(output.status.code(), Some(130));
    let state: Value =
        serde_json::from_slice(&std::fs::read(state_path).expect("final job state")).expect("decode final job state");
    assert_eq!(state["status"], "succeeded");
}

#[test]
fn mcp_subcommand_negotiates_and_publishes_the_browser_contract() {
    let root = tempfile::tempdir().expect("isolated root");
    let mut process = McpProcess::start(root.path());
    let initialize = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "browser-cli-test", "version": "1" }
        }
    }));
    assert_eq!(initialize["result"]["serverInfo"]["name"], "horizon-browser");
    process.notify(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let tools = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(32));
    assert!(tools.to_string().contains("cloud_companion_ensure_ready"));
    assert!(tools.to_string().contains("cloud_list"));
    assert!(tools.to_string().contains("browser_network_watch"));
    assert!(tools.to_string().contains("browser_http_auth"));
    assert!(tools.to_string().contains("browser_resize"));
    assert!(tools.to_string().contains("browser_screenshot"));
    assert!(tools.to_string().contains("browser_orientation"));
    assert!(tools.to_string().contains("browser_close"));
    assert!(!tools.to_string().contains("browser_ws"));
    process.close();
}

#[test]
fn successive_process_local_runs_release_ownership_immediately() {
    let home = tempfile::tempdir().expect("isolated home");
    let panel_id = "process-local-panel";
    let manifest_path = manifest::manifest_path_for_root(&home.path().join(".horizon"), panel_id);
    manifest::write_at(
        &manifest_path,
        &BrowserManifest {
            panel_local_id: panel_id.to_string(),
            ..BrowserManifest::default()
        },
    )
    .expect("write browser manifest");
    let plan = home.path().join("handoff-plan.json");
    std::fs::write(
        &plan,
        format!(
            r#"{{"version":1,"steps":[{{"id":"handoff","tool":"browser_handoff","arguments":{{"panel_id":"{panel_id}","reason":"release regression","wait":false}}}}]}}"#
        ),
    )
    .expect("write handoff plan");

    for invocation in 1..=2 {
        let output = run_process_local_command(home.path(), ["run", plan.to_str().expect("UTF-8 path")]);
        assert!(
            output.status.success(),
            "invocation {invocation} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("decode execution report");
        assert_eq!(report["ok"], true);
        let manifest: BrowserManifest =
            serde_json::from_slice(&std::fs::read(&manifest_path).expect("read released browser manifest"))
                .expect("decode released browser manifest");
        assert!(manifest.owner.is_none(), "invocation {invocation} retained its owner");
        assert!(
            manifest.handoff.is_none(),
            "invocation {invocation} retained its handoff"
        );
    }
}

#[test]
fn run_waits_for_hand_back_before_finishing_a_handoff_step() {
    let home = tempfile::tempdir().expect("isolated home");
    let panel_id = "cli-handoff-panel";
    let manifest_path = manifest::manifest_path_for_root(&home.path().join(".horizon"), panel_id);
    manifest::write_at(
        &manifest_path,
        &BrowserManifest {
            panel_local_id: panel_id.to_string(),
            ..BrowserManifest::default()
        },
    )
    .expect("write browser manifest");
    let plan = home.path().join("wait-handoff-plan.json");
    std::fs::write(
        &plan,
        format!(
            r#"{{"version":1,"steps":[{{"id":"handoff","tool":"browser_handoff","arguments":{{"panel_id":"{panel_id}","reason":"cli resume"}}}}]}}"#
        ),
    )
    .expect("write wait handoff plan");

    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().expect("UTF-8 path"), "--timeout", "10"])
        .env("HOME", home.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn waiting CLI handoff job");

    let deadline = Instant::now() + JOB_PROGRESS_GUARD;
    loop {
        if let Ok(bytes) = std::fs::read(&manifest_path)
            && let Ok(mut snapshot) = serde_json::from_slice::<BrowserManifest>(&bytes)
            && snapshot.handoff.as_ref().is_some_and(|handoff| !handoff.done)
        {
            snapshot.handoff.as_mut().expect("pending handoff").done = true;
            manifest::write_at(&manifest_path, &snapshot).expect("hand the panel back");
            break;
        }
        assert!(
            child.try_wait().expect("poll CLI handoff job").is_none(),
            "CLI handoff job exited before waiting for hand-back"
        );
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled CLI handoff job");
            child.wait().expect("reap stalled CLI handoff job");
            panic!("CLI handoff job never requested a pending handoff");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    wait_for_exit(&mut child, "CLI handoff job");
    let output = child.wait_with_output().expect("collect CLI handoff job");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("decode CLI handoff report");
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["steps"][0]["tool"], "browser_handoff");
    assert_eq!(report["steps"][0]["result"]["handoff_pending"], false);
}

fn run_command<const N: usize>(home: &std::path::Path, arguments: [&str; N]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(arguments)
        .env("HOME", home)
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .output()
        .expect("run horizon-browser")
}

fn run_process_local_command<const N: usize>(home: &std::path::Path, arguments: [&str; N]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(arguments)
        .env("HOME", home)
        .env_remove("HORIZON_BROWSER_ACTOR")
        .env("RUST_LOG", "off")
        .output()
        .expect("run process-local horizon-browser")
}

/// A job that stopped at its deadline while its blocking action was in flight.
struct DeadlineRun {
    output: std::process::Output,
    /// The deadline the job ran with.
    timeout_seconds: u64,
}

fn run_deadline_after_action(
    home: &std::path::Path,
    plan: &std::path::Path,
    manifest_path: &std::path::Path,
    output: Option<&std::path::Path>,
) -> DeadlineRun {
    let mut timeout_seconds = DEADLINE_TEST_TIMEOUT_SECONDS;
    loop {
        let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser"));
        let timeout = timeout_seconds.to_string();
        command
            .args(["run", plan.to_str().expect("UTF-8 path"), "--timeout", &timeout])
            .env("HOME", home)
            .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(output) = output {
            command.args(["--output", output.to_str().expect("UTF-8 output path")]);
        }
        let mut child = command.spawn().expect("spawn deadline browser job");
        match wait_for_queue(&mut child, manifest_path) {
            Queueing::Queued => {
                wait_for_exit_within(
                    &mut child,
                    "deadline browser job",
                    Duration::from_secs(timeout_seconds) + JOB_PROGRESS_GUARD,
                );
                return DeadlineRun {
                    output: child.wait_with_output().expect("collect deadline browser job"),
                    timeout_seconds,
                };
            }
            // Exit status 124 is the job deadline; its report goes to stdout, not stderr.
            // That run never had an action in flight, so it tests nothing.
            Queueing::Exited(status, _)
                if status.code() == Some(124) && timeout_seconds < MAX_DEADLINE_TEST_TIMEOUT_SECONDS =>
            {
                timeout_seconds *= 2;
            }
            Queueing::Exited(status, stderr) => {
                panic!("browser job exited before queueing its blocking action ({status}): {stderr}")
            }
        }
    }
}

fn write_blocking_plan(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    write_blocking_plan_with(home, "")
}

fn write_blocking_plan_with_followup(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    write_blocking_plan_with(home, r#",{"id":"followup","tool":"browser_list"}"#)
}

fn write_blocking_plan_with(home: &std::path::Path, extra_steps: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let panel_id = "blocked-panel";
    let manifest_path = manifest::manifest_path_for_root(&home.join(".horizon"), panel_id);
    manifest::write_at(
        &manifest_path,
        &BrowserManifest {
            panel_local_id: panel_id.to_string(),
            ..BrowserManifest::default()
        },
    )
    .expect("write blocking browser manifest");
    let plan = home.join("blocking-plan.json");
    std::fs::write(
        &plan,
        format!(
            r#"{{"version":1,"steps":[{{"id":"list","tool":"browser_list"}},{{"id":"snapshot","tool":"browser_snapshot","arguments":{{"panel_id":"{panel_id}","timeout_millis":60000}}}}{extra_steps}]}}"#
        ),
    )
    .expect("write blocking plan");
    (plan, manifest_path)
}

/// Answers a queued action the way the host does: the result file appears whole, so the
/// job, which polls for it, never reads a partly written result.
fn answer_action(root: &std::path::Path, panel_id: &str, result: &horizon_browser::AgentActionResult) {
    let path = manifest::action_result_path_for_root(&root.join(".horizon"), panel_id, &result.action_id);
    let directory = path.parent().expect("result directory");
    std::fs::create_dir_all(directory).expect("create result directory");
    let mut staged = tempfile::NamedTempFile::new_in(directory).expect("stage result");
    serde_json::to_writer(staged.as_file_mut(), result).expect("write staged result");
    staged.persist(&path).expect("publish result");
}

/// What a job did while the test waited for its blocking action.
enum Queueing {
    Queued,
    /// The job exited first, with this status and error output.
    Exited(std::process::ExitStatus, String),
}

fn wait_for_queue(child: &mut Child, manifest_path: &std::path::Path) -> Queueing {
    let deadline = Instant::now() + JOB_PROGRESS_GUARD;
    loop {
        if action_queued(manifest_path) {
            return Queueing::Queued;
        }
        if let Some(status) = child.try_wait().expect("poll browser job") {
            // The job may have queued its action after the read above and then exited at its
            // deadline. That run had its action in flight, and a retry would find the action
            // in the manifest before its own job queued one.
            if action_queued(manifest_path) {
                return Queueing::Queued;
            }
            let mut stderr = String::new();
            if let Some(stream) = child.stderr.as_mut() {
                stream
                    .read_to_string(&mut stderr)
                    .expect("read exited browser job error");
            }
            return Queueing::Exited(status, stderr);
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled task-owned browser job");
            child.wait().expect("reap stalled task-owned browser job");
            panic!("browser job did not queue its blocking action");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn action_queued(manifest_path: &std::path::Path) -> bool {
    std::fs::read(manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BrowserManifest>(&bytes).ok())
        .is_some_and(|manifest| !manifest.actions.is_empty())
}

fn wait_for_manifest_action(child: &mut Child, manifest_path: &std::path::Path) {
    if let Queueing::Exited(status, stderr) = wait_for_queue(child, manifest_path) {
        panic!("browser job exited before queueing its blocking action ({status}): {stderr}");
    }
}

#[cfg(unix)]
fn wait_for_job_status(child: &mut Child, jobs: &std::path::Path, expected: &str) -> std::path::PathBuf {
    let deadline = Instant::now() + JOB_PROGRESS_GUARD;
    loop {
        if let Ok(entries) = std::fs::read_dir(jobs) {
            for path in entries.flatten().map(|entry| entry.path().join("state.json")) {
                if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(state) = serde_json::from_slice::<Value>(&bytes)
                    && state["status"] == expected
                {
                    return path;
                }
            }
        }
        assert!(
            child.try_wait().expect("poll browser job").is_none(),
            "browser job exited before finalization"
        );
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled task-owned browser job");
            child.wait().expect("reap stalled task-owned browser job");
            panic!("browser job did not reach {expected} before finalization blocked");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn send_interrupt(pid: u32) {
    let status = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("send interrupt to task-owned browser job");
    assert!(status.success(), "could not interrupt task-owned browser job");
}

fn wait_for_exit(child: &mut Child, description: &str) {
    wait_for_exit_within(child, description, JOB_PROGRESS_GUARD);
}

fn wait_for_exit_within(child: &mut Child, description: &str, bound: Duration) {
    let deadline = Instant::now() + bound;
    loop {
        if child.try_wait().expect("poll task-owned browser job").is_some() {
            return;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled task-owned browser job");
            child.wait().expect("reap stalled task-owned browser job");
            panic!("{description} did not exit cooperatively");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl McpProcess {
    fn start(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
            .arg("mcp")
            .env("HOME", home)
            .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Horizon Browser MCP server");
        let stdin = child.stdin.take().expect("MCP stdin");
        let stdout = BufReader::new(child.stdout.take().expect("MCP stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn send(&mut self, message: &Value) -> Value {
        let stdin = self.stdin.as_mut().expect("open MCP stdin");
        serde_json::to_writer(&mut *stdin, message).expect("encode MCP request");
        stdin.write_all(b"\n").expect("terminate MCP request");
        stdin.flush().expect("flush MCP request");
        let mut response = String::new();
        self.stdout.read_line(&mut response).expect("read MCP response");
        serde_json::from_str(&response).expect("decode MCP response")
    }

    fn notify(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("open MCP stdin");
        serde_json::to_writer(&mut *stdin, message).expect("encode MCP notification");
        stdin.write_all(b"\n").expect("terminate MCP notification");
        stdin.flush().expect("flush MCP notification");
    }

    fn close(mut self) {
        self.stdin.take();
        let status = self.child.wait().expect("wait for MCP server");
        assert!(status.success(), "MCP server exited with {status}");
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        if self.stdin.take().is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn cli_plan_calls_the_public_usage_tool_and_preserves_multiple_provider_results() {
    let root = tempfile::tempdir().expect("root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "version":1,"steps":[{"id":"usage","tool":"browser_provider_usage"}]
        }))
        .expect("plan"),
    )
    .expect("write plan");
    let child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().expect("path")])
        .env_remove("HORIZON")
        .env_remove("HORIZON_BROWSER_ROOT")
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "horizon:usage-agent")
        .env("HORIZON_BROWSER_HOST_INSTANCE", "usage-host")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CLI");
    let queue = UsageQueue::new(root.path().join(".horizon"));
    let deadline = Instant::now() + JOB_PROGRESS_GUARD;
    loop {
        let requests = queue.claim("usage-host").expect("host queue");
        if let Some(request) = requests.first() {
            assert!(request.provider.is_none());
            let rows = ["account-a", "account-b"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| ProviderUsageSummary {
                    provider: name.to_string(),
                    supported: true,
                    local_session_limit: None,
                    running: Some(index as u64),
                    allowed: Some(4),
                    queued: Some(0),
                    sampled_at_millis: Some(12345),
                    error: None,
                })
                .collect();
            queue.complete(&request.result(rows, None)).expect("complete");
            break;
        }
        assert!(Instant::now() < deadline, "CLI did not call usage tool");
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("CLI exit");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["ok"], true);
    let providers = &report["steps"][0]["result"]["providers"];
    assert_eq!(providers[0]["provider"], "account-a");
    assert_eq!(providers[1]["provider"], "account-b");
    assert_eq!(providers[1]["running"], 1);
}

#[test]
fn standalone_cli_reports_missing_provider_host_instead_of_zero_capacity() {
    let root = tempfile::tempdir().expect("root");
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        br#"{"version":1,"steps":[{"id":"usage","tool":"browser_provider_usage"}]}"#,
    )
    .expect("plan");
    let output = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().expect("path")])
        .env_remove("HORIZON")
        .env_remove("HORIZON_BROWSER_ACTOR")
        .env_remove("HORIZON_BROWSER_HOST_INSTANCE")
        .env_remove("HORIZON_BROWSER_ROOT")
        .env("HOME", root.path())
        .output()
        .expect("CLI");
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["ok"], false);
    assert!(report.to_string().contains("requires a live Horizon host identity"));
}

#[test]
fn cli_plan_discovers_provider_devices_without_a_browser_or_configured_target() {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    std::fs::write(&plan, br#"{"version":1,"steps":[{"id":"devices","tool":"browser_provider_devices","arguments":{"provider":"account","search":"phone","offset":50}}]}"#).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", plan.to_str().unwrap()])
        .env_remove("HORIZON")
        .env_remove("HORIZON_BROWSER_ROOT")
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "horizon:catalog-agent")
        .env("HORIZON_BROWSER_HOST_INSTANCE", "catalog-host")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let queue = UsageQueue::new(root.path().join(".horizon"));
    let deadline = Instant::now() + JOB_PROGRESS_GUARD;
    loop {
        if let Some(request) = queue.claim("catalog-host").unwrap().first() {
            let query = request.catalog.as_ref().unwrap();
            assert_eq!(query.provider, "account");
            assert_eq!(query.search, "phone");
            assert_eq!(query.offset, 50);
            let mut result = request.result(vec![], None);
            result.catalog = Some(horizon_browser::provider_catalog::CatalogPage {
                total: 75,
                next_offset: None,
                devices: vec![],
            });
            queue.complete(&result).unwrap();
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["steps"][0]["result"]["catalog"]["total"], 75);
    assert_eq!(report["steps"][0]["result"]["capacity_reserved"], false);
}

#[test]
fn direct_orientation_executes_one_mcp_action_and_reports_measured_acknowledgement() {
    use horizon_browser::{
        AgentActionResult, BrowserControlAction, BrowserControlFailure, BrowserControlValue,
        remote::{OrientationSupport, RemoteOrientation, RemoteOrientationState},
    };
    for succeeds in [true, false] {
        let root = tempfile::tempdir().expect("isolated CLI home");
        let panel_id = "orientation-panel";
        let manifest_path = manifest::manifest_path_for_root(&root.path().join(".horizon"), panel_id);
        manifest::write_at(
            &manifest_path,
            &BrowserManifest {
                panel_local_id: panel_id.into(),
                remote_target: Some("synthetic-tablet".into()),
                remote_orientation: Some(RemoteOrientationState {
                    support: OrientationSupport::Supported,
                    applied: Some(RemoteOrientation::Portrait),
                }),
                ..BrowserManifest::default()
            },
        )
        .expect("mock panel");
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
            .args(["orientation", panel_id, "landscape", "--timeout-millis", "5000"])
            .env("HOME", root.path())
            .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("direct CLI");
        wait_for_manifest_action(&mut child, &manifest_path);
        let queued: BrowserManifest = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(queued.actions.len(), 1);
        let action = &queued.actions[0];
        assert!(matches!(
            action.action,
            BrowserControlAction::Orientation {
                orientation: RemoteOrientation::Landscape,
                timeout_millis: 5000
            }
        ));
        let result = if succeeds {
            AgentActionResult::completed(
                action.action_id.clone(),
                BrowserControlValue::Orientation {
                    requested: RemoteOrientation::Landscape,
                    applied: RemoteOrientation::Landscape,
                    viewport: [900, 600],
                },
            )
        } else {
            AgentActionResult::failed(
                action.action_id.clone(),
                BrowserControlFailure::new("orientation_unsupported", "mock endpoint lacks rotation"),
            )
        };
        answer_action(root.path(), panel_id, &result);
        wait_for_exit(&mut child, "direct orientation");
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["ok"], succeeds);
        assert_eq!(report["steps"].as_array().unwrap().len(), 1);
        assert_eq!(report["steps"][0]["tool"], "browser_orientation");
        if succeeds {
            assert_eq!(report["steps"][0]["result"]["applied"], "landscape");
            assert_eq!(report["steps"][0]["result"]["viewport"], json!([900, 600]));
        }
        let durable_plan: Value = serde_json::from_slice(
            &std::fs::read(std::path::Path::new(report["job_dir"].as_str().unwrap()).join("plan.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(durable_plan["steps"].as_array().unwrap().len(), 1);
        assert_eq!(durable_plan["steps"][0]["tool"], "browser_orientation");
    }
}
