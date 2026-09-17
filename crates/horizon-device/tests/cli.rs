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

#[test]
fn mcp_startup_errors_leave_stdout_as_protocol_only() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
        .args(["--target", "unused.json", "mcp"])
        .stdin(std::process::Stdio::null())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MCP transport failed"));
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
        fn initialize(&mut self) -> Result<()> {
            self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"device-test","version":"1"}}}))?;
            assert!(self.receive(1)?["result"]["capabilities"]["tools"].is_object());
            self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
            Ok(())
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
        session.initialize()?;
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
    #[test]
    #[ignore = "requires HORIZON_DEVICE_TEST_TARGET pointing to an owned virtual desktop"]
    fn cli_and_mcp_preserve_unicode_for_a_delayed_receiver() -> Result<()> {
        use x11rb::protocol::{
            Event,
            xproto::{CreateWindowAux, EventMask, InputFocus, WindowClass},
        };
        let target = std::env::var("HORIZON_DEVICE_TEST_TARGET")?;
        let config: horizon_device::Target = serde_json::from_slice(&std::fs::read(&target)?)?;
        let horizon_device::Endpoint::LocalX11 { display } = &config.endpoint else {
            return Err("requires X11".into());
        };
        let (connection, screen) = x11rb::connect(Some(display))?;
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id()?;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::KEY_PRESS),
            )?
            .check()?;
        connection.map_window(window)?.check()?;
        connection
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
            .check()?;
        let expected = "UTF8 æøå🦀 AaZz_09!?";
        for mode in ["cli", "mcp"] {
            let geometry = horizon_device::Device::connect(&config)?.screenshot()?.geometry;
            let request = json!({"geometry":geometry,"action":{"kind":"type","text":expected}});
            let target = target.clone();
            let sender = std::thread::spawn(move || -> std::result::Result<(), String> {
                send_type(mode, &target, request).map_err(|error| error.to_string())
            });
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut received = String::new();
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(30));
                while let Some(event) = connection.poll_for_event()? {
                    if let Event::KeyPress(event) = event {
                        let mapping = connection.get_keyboard_mapping(event.detail, 1)?.reply()?;
                        let symbol = mapping.keysyms.first().copied().unwrap_or_default();
                        let codepoint = if symbol & 0xff00_0000 == 0x0100_0000 {
                            symbol & 0x00ff_ffff
                        } else {
                            symbol
                        };
                        received.push(char::from_u32(codepoint).unwrap_or('\u{fffd}'));
                    }
                }
                if sender.is_finished() {
                    break;
                }
            }
            sender
                .join()
                .map_err(|_| "text sender panicked")?
                .map_err(std::io::Error::other)?;
            assert_eq!(received, expected, "{mode} keeps Unicode mappings alive until consumed");
        }
        Ok(())
    }

    fn send_type(mode: &str, target: &str, request: Value) -> Result<()> {
        if mode == "mcp" {
            let mut session = Session::start(target)?;
            session.initialize()?;
            assert_eq!(session.call(2, "device_act", request)?["isError"], false);
        } else {
            let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-device"))
                .args(["--target", target, "act", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .ok_or("missing stdin")?
                .write_all(request.to_string().as_bytes())?;
            let output = child.wait_with_output()?;
            assert!(output.status.success());
            assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?["ok"], true);
        }
        Ok(())
    }
}
