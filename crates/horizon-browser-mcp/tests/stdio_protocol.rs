use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl McpProcess {
    fn start(home: &std::path::Path) -> Self {
        Self::start_as(home, "protocol-smoke", None)
    }

    fn start_as(home: &std::path::Path, actor: &str, host_instance: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser-mcp"));
        command.env("HOME", home);
        command.env("HORIZON_BROWSER_ACTOR", actor);
        command.env("RUST_LOG", "off");
        if let Some(host_instance) = host_instance {
            command.env(horizon_core::browser::manifest::HOST_INSTANCE_ENV, host_instance);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn MCP server");
        let stdin = child.stdin.take().expect("MCP stdin");
        let stdout = BufReader::new(child.stdout.take().expect("MCP stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn handshake(&mut self) {
        let initialize = self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "horizon-test", "version": "1" }
            }
        }));
        assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
        self.notify(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    /// The tool call's error text, present when the server refused the call.
    fn error_text(response: &Value) -> String {
        let text = response["result"]["content"]
            .as_array()
            .and_then(|content| content.iter().find(|item| item["type"] == "text"))
            .and_then(|item| item["text"].as_str())
            .unwrap_or("missing error text")
            .to_string();
        format!("{text} | {response}")
    }

    fn send(&mut self, message: &Value) -> Value {
        let stdin = self.stdin.as_mut().expect("open MCP stdin");
        serde_json::to_writer(&mut *stdin, message).expect("encode MCP request");
        stdin.write_all(b"\n").expect("terminate MCP request");
        stdin.flush().expect("flush MCP request");

        let mut response = String::new();
        self.stdout.read_line(&mut response).expect("read MCP response");
        assert!(!response.is_empty(), "MCP server closed without a response");
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
fn stdio_negotiates_handshake_protocols_without_leaking_private_endpoints() {
    for (requested_version, negotiated_version) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
    ] {
        exercise_protocol(requested_version, negotiated_version);
    }
}

fn exercise_protocol(requested_version: &str, negotiated_version: &str) {
    let home = tempfile::tempdir().expect("isolated home");
    let mut process = McpProcess::start(home.path());
    let initialize = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": requested_version,
            "capabilities": {},
            "clientInfo": { "name": "horizon-test", "version": "1" }
        }
    }));
    assert_eq!(initialize["result"]["protocolVersion"], negotiated_version);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "horizon-browser");
    assert!(
        initialize["result"]["instructions"]
            .as_str()
            .is_some_and(|instructions| instructions.contains("browser_create")
                && instructions.contains("browser_network start before browser_navigate")
                && instructions.contains("browser_network_watch")
                && instructions.contains("browser_video")
                && instructions.contains("browser_visibility")
                && instructions.contains("browser_close")
                && instructions.contains("allow_additional=true")
                && instructions.contains("original panel")),
        "server instructions must teach creation and network capture workflows"
    );
    process.notify(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    let tools = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    assert_listed_tools_keep_the_browser_contract(&tools);

    let list = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "browser_list", "arguments": {} }
    }));
    assert_eq!(list["result"]["isError"], false);
    assert_eq!(list["result"]["structuredContent"]["panels"], json!([]));
    assert!(!list.to_string().contains("browser_ws"));

    process.close();
}

fn listed_tool<'a>(tools: &'a Value, name: &str) -> &'a Value {
    tools["result"]["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
        .unwrap_or_else(|| panic!("{name} tool"))
}

fn assert_listed_tools_keep_the_browser_contract(tools: &Value) {
    let encoded_tools = tools.to_string();
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(16));
    let create = listed_tool(tools, "browser_create");
    let target = &create["inputSchema"]["properties"]["target"];
    assert!(
        target["description"]
            .as_str()
            .is_some_and(|description| description.contains("Configured remote target name")),
        "browser_create accepts a configured remote target name: {target}"
    );
    assert!(
        !create["inputSchema"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|name| name == "target")),
        "target stays optional"
    );
    for axis in ["width", "height"] {
        let schema = &create["inputSchema"]["properties"][axis];
        assert!(
            schema["description"]
                .as_str()
                .is_some_and(|description| description.contains("320-8000")),
            "{axis} documents the shared viewport bounds"
        );
        assert!(
            !create["inputSchema"]["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|name| name == axis)),
            "{axis} stays optional so an omitted axis keeps Horizon's default size"
        );
    }
    assert!(
        create["description"]
            .as_str()
            .is_some_and(|description| description.contains("browser_list is empty")
                && description.contains("never create a helper panel")
                && description.contains("allow_additional=true"))
    );
    assert!(create["inputSchema"].to_string().contains("allow_additional"));
    let network = listed_tool(tools, "browser_network");
    assert!(
        network["description"]
            .as_str()
            .is_some_and(|description| description.contains("tail -f"))
    );
    assert!(network["inputSchema"].to_string().contains("Start only"));
    let video = listed_tool(tools, "browser_video");
    assert!(
        video["description"]
            .as_str()
            .is_some_and(|description| description.contains("WebM") && description.contains("pause"))
    );
    assert!(video["inputSchema"].to_string().contains("Start only"));
    let watch = listed_tool(tools, "browser_network_watch");
    assert!(watch["description"].as_str().is_some_and(|description| {
        description.contains("next_sequence") && description.contains("no capture path")
    }));
    let visibility = listed_tool(tools, "browser_visibility");
    assert!(
        visibility["description"]
            .as_str()
            .is_some_and(|description| description.contains("without stopping"))
    );
    let close = listed_tool(tools, "browser_close");
    assert!(
        close["description"]
            .as_str()
            .is_some_and(|description| description.contains("stop its session") && description.contains("remote"))
    );
    assert!(close["inputSchema"].to_string().contains("panel_id"));
    let wait = listed_tool(tools, "browser_wait");
    assert!(
        wait["description"]
            .as_str()
            .is_some_and(|description| description.contains("browser_unavailable"))
    );
    let audit = listed_tool(tools, "browser_audit");
    assert!(audit["description"].as_str().is_some_and(|description| {
        description.contains("next_event_id")
            && description.contains("from_start")
            && description.contains("older_records_dropped")
    }));
    assert!(audit["inputSchema"].to_string().contains("after_event_id"));
    assert!(!encoded_tools.contains("browser_ws"));
    assert!(!encoded_tools.contains("manifest_path"));
}

/// The fake host side of a create/resize roundtrip: wait for the queued
/// request the MCP server wrote under the child's HOME, check it, publish a
/// manifest and the typed result, the way the real host does.
const FAKE_HOST: &str = "protocol-host";
const FAKE_ACTOR: &str = "horizon:protocol-agent";
const FAKE_PANEL: &str = "browser-smoke";

fn wait_for_queue_request(directory: &std::path::Path) -> (std::path::PathBuf, Value) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(entry) = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".request.json"))
        {
            let raw = std::fs::read_to_string(entry.path()).expect("read queued request");
            return (entry.path(), serde_json::from_str(&raw).expect("decode queued request"));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the MCP server never queued a request in {}",
            directory.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn write_json(path: &std::path::Path, value: &Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create runtime directory");
    }
    std::fs::write(path, value.to_string()).expect("write runtime file");
}

#[test]
fn an_injected_agent_roundtrips_create_size_through_the_host_queue() {
    let home = tempfile::tempdir().expect("isolated home");
    let root = home.path().join(".horizon");
    let manifest_path = horizon_core::browser::manifest::manifest_path_for_root(&root, FAKE_PANEL);
    let mut process = McpProcess::start_as(home.path(), FAKE_ACTOR, Some(FAKE_HOST));
    process.handshake();

    let home = home.path().to_path_buf();
    let host = std::thread::spawn(move || {
        // Create phase: the requested viewport travels with the request.
        let (request_path, request) = wait_for_queue_request(&home.join(".horizon/runtime/browser-create"));
        assert_eq!(request["actor"], FAKE_ACTOR);
        assert_eq!(request["host_instance"], FAKE_HOST);
        assert_eq!(request["width"], 1280, "width travels in the create request");
        assert_eq!(request["height"], 800, "height travels in the create request");
        horizon_core::browser::manifest::write_at(
            &manifest_path,
            &horizon_core::browser::manifest::BrowserManifest {
                panel_local_id: FAKE_PANEL.to_string(),
                host: Some(FAKE_HOST.to_string()),
                workspace: Some(horizon_core::browser::manifest::ManifestWorkspace::new(
                    FAKE_HOST,
                    "workspace-1",
                    vec![FAKE_ACTOR.to_string()],
                )),
                owner: Some(horizon_core::browser::manifest::ManifestOwner {
                    name: FAKE_ACTOR.to_string(),
                    tty: None,
                    updated_at: horizon_core::browser::manifest::now_millis(),
                }),
                viewport: Some([1280, 800]),
                ..horizon_core::browser::manifest::BrowserManifest::default()
            },
        )
        .expect("publish created panel manifest");
        write_json(
            &request_path.with_file_name(format!(
                "{}.result.json",
                request_path
                    .file_name()
                    .expect("request name")
                    .to_string_lossy()
                    .strip_suffix(".request.json")
                    .expect("request suffix")
            )),
            &json!({
                "request_id": request["request_id"],
                "actor": request["actor"],
                "outcome": {
                    "status": "ready",
                    "panel_local_id": FAKE_PANEL,
                    "navigation": "not_requested",
                    "startup_millis": 3
                }
            }),
        );
        std::fs::remove_file(request_path).expect("consume create request");
    });

    let create = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "browser_create",
            "arguments": {
                "width": 1280,
                "height": 800,
                "allow_additional": true,
                "timeout_millis": 15000
            }
        }
    }));
    assert_eq!(create["result"]["structuredContent"]["panel"]["panel_id"], FAKE_PANEL);
    assert_eq!(
        create["result"]["structuredContent"]["panel"]["width"], 1280,
        "the created panel reports the host-stamped viewport"
    );
    assert_eq!(create["result"]["structuredContent"]["panel"]["height"], 800);

    let list = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "browser_list", "arguments": {} }
    }));
    assert_eq!(
        list["result"]["structuredContent"]["panels"][0]["width"], 1280,
        "browser_list keeps reporting the host-stamped viewport"
    );
    assert_eq!(list["result"]["structuredContent"]["panels"][0]["height"], 800);

    host.join().expect("fake host");
    process.close();
}

#[test]
fn viewport_requests_are_refused_before_any_host_sees_them() {
    let home = tempfile::tempdir().expect("isolated home");
    let mut process = McpProcess::start_as(home.path(), FAKE_ACTOR, Some(FAKE_HOST));
    process.handshake();

    // A remote target plus a size is refused before the queue is written.
    let remote = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "browser_create",
            "arguments": { "target": "ios", "width": 1280, "allow_additional": true }
        }
    }));
    let text = McpProcess::error_text(&remote);
    assert!(
        text.contains("fixed device viewport"),
        "remote targets with a size fail with the fixed-viewport error: {text}"
    );
    // An out-of-range size is refused before the queue is written.
    let small = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "browser_create",
            "arguments": { "width": 100, "allow_additional": true }
        }
    }));
    assert!(
        McpProcess::error_text(&small).contains("between 320 and 8000"),
        "out-of-range create sizes are refused with the bounds"
    );
    assert!(
        !home.path().join(".horizon/runtime/browser-create").exists(),
        "refused requests never reach the host queue"
    );
    process.close();
}
