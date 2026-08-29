use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use horizon_core::browser::manifest::{self, BrowserManifest};
use serde_json::{Value, json};

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
    assert_eq!(file_report, stdout_report);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(report).expect("report metadata").permissions().mode() & 0o777,
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
