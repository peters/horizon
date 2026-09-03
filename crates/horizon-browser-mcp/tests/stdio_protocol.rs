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
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser-mcp"))
            .env("HOME", home)
            .env("HORIZON_BROWSER_ACTOR", "protocol-smoke")
            .env("RUST_LOG", "off")
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
fn stdio_negotiates_legacy_and_current_protocols_without_leaking_private_endpoints() {
    for protocol_version in ["2025-06-18", "2026-07-28"] {
        exercise_protocol(protocol_version);
    }
}

fn exercise_protocol(protocol_version: &str) {
    let home = tempfile::tempdir().expect("isolated home");
    let mut process = McpProcess::start(home.path());
    let initialize = process.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": { "name": "horizon-test", "version": "1" }
        }
    }));
    assert_eq!(initialize["result"]["protocolVersion"], protocol_version);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "horizon-browser");
    assert!(
        initialize["result"]["instructions"]
            .as_str()
            .is_some_and(|instructions| instructions.contains("browser_create")
                && instructions.contains("browser_network start before browser_navigate")
                && instructions.contains("browser_network_watch")
                && instructions.contains("browser_visibility")
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
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(14));
    let create = listed_tool(tools, "browser_create");
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
