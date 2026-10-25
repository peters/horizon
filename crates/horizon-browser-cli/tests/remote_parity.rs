//! A local profile host and a profile host of another computer advertise the
//! same capabilities to an agent, and a tool refuses with a listed reason.
//!
//! The second host is reached through a real `ssh` transport: the test starts
//! one profile host in a temporary runtime root and connects to it twice, once
//! as a `mcp --connect` client of this computer and once through the bridge
//! `mcp --connect --remote <target>`, which runs `ssh -Tq <target>` and the same
//! server on the other end. The default target is `127.0.0.1`, which is a real
//! SSH transport with no virtual machine.
//!
//! The test needs an SSH target that answers without a prompt and the
//! `horizon-browser` binary of this build on that machine. `HORIZON_BROWSER_REMOTE_SSH_TARGET`
//! names another target, and `HORIZON_BROWSER_REMOTE_EXECUTABLE` names the binary
//! when the login shell of the target does not find it on its `PATH`. Without a
//! reachable target the test states the reason and does nothing, because a
//! machine without an SSH server can not show this property.
//!
//! The live test at the end also needs a local Chromium; run it with
//! `cargo test -p horizon-browser-cli --test remote_parity -- --ignored`.
//!
//! Unix only: the profile host checks the mode of its host file and stops on a signal.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HOST_FILE: &str = "browser:\n  profiles:\n    mail:\n      backend: chromium\n      args: [\"--disable-gpu\"]\n";
const ACTOR: &str = "horizon:parity";

fn ssh_target() -> String {
    std::env::var("HORIZON_BROWSER_REMOTE_SSH_TARGET")
        .ok()
        .filter(|target| !target.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn ssh_reachable(target: &str) -> bool {
    Command::new("ssh")
        .args(["-Tq", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(target)
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn host_file(root: &Path) -> PathBuf {
    let path = root.join("host.yaml");
    std::fs::write(&path, HOST_FILE).expect("write host file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
    path
}

/// A running profile host. A test that fails before it stops the host still
/// stops it here, so that no host and no browser of the test stays.
struct Host(Option<Child>);

impl Host {
    fn id(&self) -> u32 {
        self.0.as_ref().map_or(0, Child::id)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
            let deadline = Instant::now() + Duration::from_secs(30);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn host(root: &Path, config: &Path) -> Host {
    let child = Command::new(env!("CARGO_BIN_EXE_horizon-browser"))
        .args(["host", "--config"])
        .arg(config)
        .env("HOME", root)
        .env("HORIZON_BROWSER_ROOT", root.join("state"))
        .env_remove("HORIZON_BROWSER_CONFIG")
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the profile host");
    Host(Some(child))
}

/// The record that the host writes when it serves requests.
fn instance(root: &Path, pid: u32) -> String {
    let path = root.join("state").join("profile-host.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(record) = std::fs::read(&path)
            && let Ok(record) = serde_json::from_slice::<Value>(&record)
            && record["pid"] == json!(pid)
            && let Some(instance) = record["host_instance"].as_str()
        {
            return instance.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "the host wrote no record at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One MCP client of the profile host: a plain `mcp --connect` client of this
/// computer, or the bridge that reaches the same host over SSH.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

#[derive(Clone, Copy)]
enum Transport<'a> {
    Local,
    Remote(&'a str),
}

impl Client {
    fn start(transport: Transport<'_>, root: &Path, instance: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser"));
        command.args(["mcp", "--connect", "--inline-screenshots"]);
        if let Transport::Remote(target) = transport {
            command.args(["--remote", target]);
            // The bridge reads the host instance of the remote record itself.
            // The remote server runs the binary of this build with the temporary
            // runtime root of this test, so the test never reads the state of
            // the person.
            command.env_remove("HORIZON_BROWSER_HOST_INSTANCE");
            command.env_remove("HORIZON_BROWSER_ROOT");
            command.env("HORIZON_BROWSER_REMOTE_ROOT", root.join("state"));
            command.env(
                "HORIZON_BROWSER_REMOTE_EXECUTABLE",
                env!("CARGO_BIN_EXE_horizon-browser"),
            );
        } else {
            command.env("HORIZON_BROWSER_ROOT", root.join("state"));
            command.env("HORIZON_BROWSER_HOST_INSTANCE", instance);
        }
        let mut child = command
            .env("HORIZON_BROWSER_ACTOR", ACTOR)
            .env_remove("HORIZON_BROWSER_CONFIG")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the MCP client");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut client = Self {
            child,
            stdin,
            stdout,
            next: 1,
        };
        client.call(
            "initialize",
            &json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "parity", "version": "1"}}),
        );
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn call(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).expect("read") > 0,
                "the client closed stdout before the answer to {method}"
            );
            let message: Value = serde_json::from_str(&line).expect("json-rpc");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// The tools of the server, with their schemas.
    fn tools(&mut self) -> Value {
        let reply = self.call("tools/list", &json!({}));
        reply["result"]["tools"].clone()
    }

    /// The tool's structured result, or its error text.
    fn tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let reply = self.call("tools/call", &json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default().to_owned();
        if result["isError"] == true {
            return Err(text);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// The content blocks of a tool result, after the structured text.
    fn call_blocks(&mut self, name: &str, arguments: &Value) -> Value {
        let reply = self.call("tools/call", &json!({"name": name, "arguments": arguments}));
        reply["result"]["content"].clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(30);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tool_names(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_profile_host_over_ssh_advertises_the_same_capabilities_as_a_local_connect_client() {
    let target = ssh_target();
    if !ssh_reachable(&target) {
        eprintln!(
            "skipped: `ssh -Tq {target}` needs an SSH server that answers without a prompt, \
             with the horizon-browser binary of this build on that machine"
        );
        return;
    }
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path());
    let running = host(root.path(), &config);
    let instance = instance(root.path(), running.id());

    let mut local = Client::start(Transport::Local, root.path(), &instance);
    let mut remote = Client::start(Transport::Remote(&target), root.path(), &instance);
    let (local_tools, remote_tools) = (local.tools(), remote.tools());
    assert_eq!(
        local_tools, remote_tools,
        "the bridge advertises the capabilities of the host it reaches"
    );

    let names = tool_names(&local_tools);
    for expected in [
        "browser_list",
        "browser_create",
        "browser_act",
        "browser_video",
        "browser_network",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} is advertised: {names:?}"
        );
    }
    let act = local_tools
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == json!("browser_act")))
        .expect("browser_act");
    assert!(
        serde_json::to_string(act).expect("schema").contains("set_files"),
        "set_files is part of the comparable contract"
    );

    // A page needs a profile, so a create without a target refuses on both sides
    // with the same listed reason.
    let local_error = local
        .tool("browser_create", &json!({"allow_additional": true}))
        .expect_err("a profile host refuses a create without a target");
    let remote_error = remote
        .tool("browser_create", &json!({"allow_additional": true}))
        .expect_err("the bridge keeps the refusal of the host");
    // The action id of each call differs, so the test compares the listed reason.
    let listed_reason = |text: &str| {
        text.find("target_required")
            .map_or_else(|| text.to_string(), |at| text[at..].to_string())
    };
    assert!(
        listed_reason(&local_error).starts_with("target_required"),
        "the refusal names its reason: {local_error}"
    );
    assert_eq!(
        listed_reason(&local_error),
        listed_reason(&remote_error),
        "the same refusal reason reaches the agent over the bridge"
    );

    // Both clients see the same, empty page set of one actor on one host.
    let pages = remote.tool("browser_list", &json!({})).expect("browser_list");
    assert_eq!(
        pages,
        local.tool("browser_list", &json!({})).expect("browser_list"),
        "one identity sees one page set"
    );

    drop(running);
}

/// The bridge stages a local file on the remote host before the call, and the
/// remote staging folder is gone when the call answers.
#[test]
#[ignore = "opens real pages in a local Chromium and needs an SSH target"]
fn the_bridge_stages_a_local_file_on_the_remote_host_before_set_files() {
    let target = ssh_target();
    assert!(
        ssh_reachable(&target),
        "`ssh -Tq {target}` must answer without a prompt for this test"
    );
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path());
    let running = host(root.path(), &config);
    let instance = instance(root.path(), running.id());

    let work = root.path().join("work");
    std::fs::create_dir_all(&work).expect("work root");
    let upload = work.join("report 2026.txt");
    std::fs::write(&upload, b"parity").expect("local file");

    let mut remote = Client::start(Transport::Remote(&target), root.path(), &instance);
    let page = remote
        .tool(
            "browser_create",
            &json!({"target": "mail", "url": "data:text/html,<input type=file id=f>"}),
        )
        .expect("create a page of the mail profile");
    let panel = page["panel_id"].as_str().expect("panel id").to_string();
    let node = remote
        .tool("browser_query", &json!({"panel_id": panel, "selector": "#f"}))
        .expect("query the file input");
    let reference = node["nodes"][0]["ref"].as_str().expect("a ref").to_string();

    let attached = remote
        .tool(
            "browser_act",
            &json!({"panel_id": panel, "action": "set_files", "ref": reference,
                    "files": [upload.to_string_lossy()]}),
        )
        .expect("set_files through the bridge");
    let names = serde_json::to_string(&attached).expect("result");
    assert!(
        names.contains("report 2026.txt"),
        "the page holds the file of this computer: {names}"
    );

    let staging = root.path().join("state").join("browser-staging");
    let left = std::fs::read_dir(&staging)
        .expect("the bridge staged the file under the runtime root of the remote host")
        .count();
    assert_eq!(left, 0, "the staged copy is removed after the call");
    drop(running);
}

/// A bounded network capture of the remote page arrives in the tool result.
#[test]
#[ignore = "opens real pages in a local Chromium and needs an SSH target"]
fn the_bridge_returns_a_bounded_network_capture_inline() {
    let target = ssh_target();
    assert!(
        ssh_reachable(&target),
        "`ssh -Tq {target}` must answer without a prompt for this test"
    );
    let root = tempfile::tempdir().expect("root");
    let config = host_file(root.path());
    let running = host(root.path(), &config);
    let instance = instance(root.path(), running.id());

    let mut remote = Client::start(Transport::Remote(&target), root.path(), &instance);
    let page = remote
        .tool(
            "browser_create",
            &json!({"target": "mail", "url": "data:text/html,<p>network</p>"}),
        )
        .expect("create a page of the mail profile");
    let panel = page["panel_id"].as_str().expect("panel id").to_string();
    remote
        .tool(
            "browser_network",
            &json!({"panel_id": panel, "operation": "start", "include_http": true}),
        )
        .expect("start a capture");
    remote
        .tool(
            "browser_evaluate",
            &json!({"panel_id": panel, "expression": "document.title"}),
        )
        .expect("one action while the capture runs");
    let blocks = remote.call_blocks("browser_network", &json!({"panel_id": panel, "operation": "stop"}));

    let capture = blocks
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["type"] == json!("resource")))
        .expect("the bridge adds the capture of the remote host to the result");
    assert_eq!(capture["resource"]["mimeType"], json!("application/x-ndjson"));
    assert!(
        capture["resource"]["blob"]
            .as_str()
            .is_some_and(|blob| !blob.is_empty()),
        "the bytes of the capture cross the SSH stream"
    );
    drop(running);
}
