#![cfg(feature = "cli")]

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
}

impl Session {
    fn start(target: &std::path::Path) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
            .arg("--target")
            .arg(target)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("MCP stdin missing")?;
        let stdout = child.stdout.take().ok_or("MCP stdout missing")?;
        let (sender, responses) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            responses,
        })
    }

    fn write(&mut self, value: &Value) -> Result<()> {
        let stdin = self.stdin.as_mut().ok_or("MCP stdin closed")?;
        serde_json::to_writer(&mut *stdin, value)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, value: &Value) -> Result<Value> {
        self.write(value)?;
        Ok(serde_json::from_str(
            &self.responses.recv_timeout(Duration::from_secs(10))?,
        )?)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn tool_catalog_supports_discovery_and_legacy_handshakes() -> Result<()> {
    let home = tempfile::tempdir()?;
    // An absent target makes the read-only call safe on every test platform.
    let target = home.path().join("absent-target.json");
    for discover in [true, false] {
        let mut session = Session::start(&target)?;
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        let params = if discover { json!({"_meta": meta}) } else { json!({}) };
        if discover {
            let response = session.request(&json!({
                "jsonrpc":"2.0", "id":1, "method":"server/discover", "params":params
            }))?;
            assert!(response.get("error").is_none(), "{response}");
        } else {
            let response = session.request(&json!({
                "jsonrpc":"2.0", "id":1, "method":"initialize",
                "params":{"protocolVersion":"2026-07-28", "capabilities":{},
                    "clientInfo":{"name":"protocol-test", "version":"1"}}
            }))?;
            assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
            session.write(&json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))?;
        }
        let response = session.request(&json!({
            "jsonrpc":"2.0", "id":2, "method":"tools/list", "params":params
        }))?;
        assert_eq!(response["result"]["ttlMs"], 0);
        assert_eq!(response["result"]["cacheScope"], "private");
        let tools = response["result"]["tools"].as_array().ok_or("missing tools")?;
        assert_eq!(tools.len(), 5);
        assert!(tools.iter().any(|tool| tool["name"] == "device_doctor"));
        let mut call = params;
        call["name"] = json!("device_doctor");
        call["arguments"] = json!({});
        let response = session.request(&json!({
            "jsonrpc":"2.0", "id":3, "method":"tools/call", "params":call
        }))?;
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["isError"], true);
    }
    Ok(())
}
