#![cfg(feature = "cli")]
use serde_json::{Value, json};
use std::process::Command;

#[test]
fn malformed_actions_and_unavailable_targets_fail_structurally() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target.json");
    std::fs::write(
        &target,
        json!({"id":"absent", "endpoint":{"kind":"local_x11","display":":999999"}}).to_string(),
    )?;
    for command in [vec!["doctor"], vec!["act", "{}"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
            .arg("--target")
            .arg(&target)
            .args(command)
            .output()?;
        assert!(!result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(value["ok"], false);
        assert!(value["error"]["code"].is_string());
    }
    Ok(())
}

#[test]
fn cooperating_process_lock_prevents_backend_access() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target.json");
    std::fs::write(&target, "not even a valid config")?;
    let lock = std::fs::File::create(target.with_extension("lock"))?;
    lock.lock()?;
    let result = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
        .arg("--target")
        .arg(&target)
        .arg("doctor")
        .output()?;
    let value: Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(value["ok"], false);
    assert!(value["error"]["message"].as_str().is_some_and(|m| m.contains("busy")));
    Ok(())
}

#[cfg(target_os = "linux")]
mod live_mcp {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Child, ChildStdin, Stdio},
        sync::mpsc::{Receiver, channel},
        time::{Duration, Instant},
    };
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    struct Session {
        child: Child,
        input: ChildStdin,
        output: Receiver<Value>,
    }
    impl Session {
        fn start(target: &str) -> Result<Self> {
            let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
                .args(["--target", target, "mcp"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let input = child.stdin.take().ok_or("missing stdin")?;
            let stdout = child.stdout.take().ok_or("missing stdout")?;
            let (sender, output) = channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                    if let Ok(value) = serde_json::from_str(&line)
                        && sender.send(value).is_err()
                    {
                        break;
                    }
                }
            });
            Ok(Self { child, input, output })
        }
        fn send(&mut self, value: Value) -> Result<()> {
            writeln!(self.input, "{value}")?;
            self.input.flush()?;
            Ok(())
        }
        fn receive(&self, id: u32) -> Result<Value> {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let message = self
                    .output
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
                if message["id"] == id {
                    return Ok(message);
                }
            }
        }
        fn call(&mut self, id: u32, tool: &str, arguments: Value) -> Result<Value> {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":tool,"arguments":arguments}}))?;
            Ok(self.receive(id)?["result"].clone())
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    #[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned virtual desktop"]
    fn stdio_images_errors_and_cancelled_input_preserve_the_contract() -> Result<()> {
        let target = std::env::var("HORIZON_DEVICE_TEST_TARGET")?;
        let config: horizon_device::Target = serde_json::from_slice(&std::fs::read(&target)?)?;
        let horizon_device::Endpoint::LocalX11 { display } = &config.endpoint else {
            return Err("requires X11".into());
        };
        let (connection, screen) = x11rb::connect(Some(display))?;
        let root = connection.setup().roots[screen].root;
        let mut session = Session::start(&target)?;
        session.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"device-test","version":"1"}}}))?;
        assert!(session.receive(1)?["result"]["capabilities"]["tools"].is_object());
        session.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        let image = session.call(2, "device_screenshot", json!({}))?;
        assert_eq!(image["isError"], false);
        assert_eq!(image["content"][1]["type"], "image");
        assert_eq!(image["content"][1]["mimeType"], "image/png");
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(image["content"][1]["data"].as_str().ok_or("missing image")?)?;
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let receipt: Value = serde_json::from_str(image["content"][0]["text"].as_str().ok_or("missing receipt")?)?;
        assert!(receipt["result"].get("image_base64").is_none());
        let geometry = receipt["result"]["geometry"].clone();
        let point = json!({"x":1100,"y":180});
        let action = json!({"kind":"drag","from":point,"to":point,"duration_ms":19});
        let started = Instant::now();
        assert_eq!(
            session.call(3, "device_act", json!({"geometry":geometry,"action":action}))?["isError"],
            false
        );
        assert!(started.elapsed() >= Duration::from_millis(19));
        let mut stale = geometry.clone();
        stale["revision"] = json!("stale");
        let error = session.call(4, "device_act", json!({"geometry":stale,"action":action}))?;
        assert_eq!(error["isError"], true);
        let error: Value = serde_json::from_str(error["content"][0]["text"].as_str().ok_or("missing error")?)?;
        assert_eq!(error["error"]["code"], "stale_geometry");
        session.send(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
            "name":"device_act","arguments":{"geometry":geometry,"action":{
                "kind":"drag","from":point,"to":point,"duration_ms":1500}}}}))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while u16::from(connection.query_pointer(root)?.reply()?.mask) & 0x100 == 0 {
            assert!(Instant::now() < deadline, "drag never pressed its button");
            std::thread::sleep(Duration::from_millis(5));
        }
        session.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5,"reason":"test cancellation"}}))?;
        while u16::from(connection.query_pointer(root)?.reply()?.mask) & 0x100 != 0 {
            assert!(Instant::now() < deadline, "cancelled drag left its button pressed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.call(6, "device_screenshot", json!({}))?["isError"], false);
        Ok(())
    }
}
