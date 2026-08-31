use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{self, BrowserManifest};
use serde_json::{Value, json};

const DEADLINE_TEST_TIMEOUT_SECONDS: u64 = 3;

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
    assert_eq!(state["version"], 1);
    assert_eq!(state["status"], "succeeded");
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
    let jobs = root.path().join(".horizon/browser-jobs");
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
    let output = run_deadline_after_action(root.path(), &plan, &manifest_path, None);

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
    assert_eq!(state["execution_timeout_seconds"], DEADLINE_TEST_TIMEOUT_SECONDS);
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
    );
    assert_eq!(failed_output.status.code(), Some(124));
    assert!(String::from_utf8_lossy(&failed_output.stderr).contains("could not open report"));
}

#[test]
fn run_deadline_bounds_open_stdin_before_durable_setup() {
    let root = tempfile::tempdir().expect("isolated root");
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["run", "-", "--timeout", "1"])
        .env("HOME", root.path())
        .env("HORIZON_BROWSER_ACTOR", "browser-cli-test")
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin browser job");
    let stdin = child.stdin.take().expect("open child stdin");

    let started = Instant::now();
    let wait_deadline = started + Duration::from_secs(5);
    loop {
        if child.try_wait().expect("poll stdin browser job").is_some() {
            break;
        }
        if Instant::now() >= wait_deadline {
            drop(stdin);
            child.kill().expect("kill stalled task-owned browser job");
            child.wait().expect("reap stalled task-owned browser job");
            panic!("browser job did not enforce its deadline while stdin remained open");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("collect stdin browser job output");

    assert_eq!(output.status.code(), Some(124));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("job deadline exceeded"));
    assert!(!stderr.contains("in-flight browser action"));
    assert!(!root.path().join(".horizon/browser-jobs").exists());
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
            "protocolVersion": "2026-07-28",
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
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(14));
    assert!(tools.to_string().contains("browser_network_watch"));
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
            r#"{{"version":1,"steps":[{{"id":"handoff","tool":"browser_handoff","arguments":{{"panel_id":"{panel_id}","reason":"release regression"}}}}]}}"#
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

fn run_deadline_after_action(
    home: &std::path::Path,
    plan: &std::path::Path,
    manifest_path: &std::path::Path,
    output: Option<&std::path::Path>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser"));
    let timeout = DEADLINE_TEST_TIMEOUT_SECONDS.to_string();
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
    wait_for_manifest_action(&mut child, manifest_path);
    child.wait_with_output().expect("collect deadline browser job")
}

fn write_blocking_plan(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
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
            r#"{{"version":1,"steps":[{{"id":"list","tool":"browser_list"}},{{"id":"snapshot","tool":"browser_snapshot","arguments":{{"panel_id":"{panel_id}","timeout_millis":60000}}}}]}}"#
        ),
    )
    .expect("write blocking plan");
    (plan, manifest_path)
}

fn wait_for_manifest_action(child: &mut Child, manifest_path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = std::fs::read(manifest_path)
            && let Ok(manifest) = serde_json::from_slice::<BrowserManifest>(&bytes)
            && !manifest.actions.is_empty()
        {
            return;
        }
        assert!(
            child.try_wait().expect("poll browser job").is_none(),
            "browser job exited before queueing its blocking action"
        );
        if Instant::now() >= deadline {
            child.kill().expect("kill stalled task-owned browser job");
            child.wait().expect("reap stalled task-owned browser job");
            panic!("browser job did not queue its blocking action");
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
