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
    let encoded_tools = tools.to_string();
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(10));
    assert!(!encoded_tools.contains("browser_ws"));
    assert!(!encoded_tools.contains("manifest_path"));

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
