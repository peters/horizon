//! Headless cross-origin form acceptance through the public MCP stdio tools.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use horizon_browser::{
    BackendKind, BrowserCommand, BrowserConfig, BrowserControlAction, BrowserControlValue, BrowserSessionConfig,
    BrowserTarget, FrameSlot, start_session,
};
use horizon_browser_control::manifest::ManifestCoordination;
use serde_json::{Value, json};

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}
impl McpProcess {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser-mcp"))
            .env("HORIZON_BROWSER_ROOT", root)
            .env("HORIZON_BROWSER_ACTOR", "frame-test")
            .env_remove("HORIZON_BROWSER_HOST_INSTANCE")
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut process = Self {
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 1,
        };
        process.request("initialize", &json!({"protocolVersion":"2025-06-18", "capabilities":{}, "clientInfo":{"name":"frame-test","version":"1"}}));
        process.write(&json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
        process
    }
    fn write(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, message).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
        let mut response = String::new();
        self.stdout.read_line(&mut response).unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], id);
        response["result"].clone()
    }
    fn call(&mut self, tool: &str, mut arguments: Value) -> Result<Value, String> {
        arguments["panel_id"] = json!("frame-test");
        let result = self.request("tools/call", &json!({"name":tool,"arguments":arguments}));
        if result["isError"] == true {
            return Err(format!("{tool}: {result}"));
        }
        Ok(result["structuredContent"].clone())
    }
    fn action(&mut self, action: BrowserControlAction) -> Result<BrowserControlValue, String> {
        match action {
            BrowserControlAction::Snapshot { max_nodes } => {
                let value = self.call("browser_snapshot", json!({"max_nodes":max_nodes}))?;
                let snapshot = serde_json::from_value(value).map_err(|error| format!("{error}"))?;
                Ok(BrowserControlValue::Snapshot { snapshot })
            }
            BrowserControlAction::Query { selector, max_results } => {
                let value = self.call("browser_query", json!({"selector":selector,"max_results":max_results}))?;
                Ok(BrowserControlValue::Nodes {
                    generation: value["generation"].as_u64().unwrap(),
                    revision: value["revision"].as_u64().unwrap(),
                    nodes: serde_json::from_value(value["nodes"].clone()).unwrap(),
                })
            }
            BrowserControlAction::Fill {
                target: BrowserTarget::Ref { reference },
                value,
            } => {
                self.call("browser_act", json!({"action":"fill","ref":reference,"value":value}))?;
                Ok(BrowserControlValue::Accepted)
            }
            BrowserControlAction::Click {
                target: BrowserTarget::Ref { reference },
                count,
            } => {
                self.call("browser_act", json!({"action":"click","ref":reference,"count":count}))?;
                Ok(BrowserControlValue::Accepted)
            }
            BrowserControlAction::Evaluate { expression, .. } => {
                let value = self.call("browser_evaluate", json!({"expression":expression}))?;
                Ok(BrowserControlValue::Json {
                    value: value["value"].clone(),
                })
            }
            BrowserControlAction::WaitForSelector {
                selector,
                state,
                timeout_millis,
            } => {
                self.call(
                    "browser_wait",
                    json!({"selector":selector,"state":state,"timeout_millis":timeout_millis}),
                )?;
                Ok(BrowserControlValue::Accepted)
            }
            _ => panic!("unsupported test action"),
        }
    }
    fn success(&mut self, action: BrowserControlAction) -> BrowserControlValue {
        self.action(action).unwrap()
    }
    fn nodes(&mut self, selector: &str) -> Vec<horizon_browser::BrowserNode> {
        let BrowserControlValue::Nodes { nodes, .. } = self.success(BrowserControlAction::Query {
            selector: selector.into(),
            max_results: 100,
        }) else {
            panic!("nodes")
        };
        nodes
    }
}
impl Drop for McpProcess {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires local Chromium, Firefox, geckodriver and loopback sockets"]
fn mcp_cross_origin_form_roundtrip_and_staleness_on_both_backends() {
    let root = tempfile::tempdir().unwrap();
    horizon_browser_control::paths::configure_runtime_root(root.path()).unwrap();
    for backend in [BackendKind::ChromiumCdp, BackendKind::FirefoxBidi] {
        for nested in [false, true] {
            eprintln!("Testing {backend:?}, nested={nested}");
            run(backend, root.path(), nested);
        }
    }
}

fn wait_for_first_frame(session: &horizon_browser::BrowserSession, slot: &FrameSlot) {
    let ready = Instant::now() + Duration::from_secs(25);
    let mut startup_warnings = Vec::new();
    while slot.latest().is_none() {
        while let Ok(event) = session.event_rx.try_recv() {
            if let horizon_browser::BrowserEvent::Warning(warning) = event
                && startup_warnings.len() < 8
            {
                startup_warnings.push(warning);
            }
        }
        assert!(Instant::now() < ready, "browser startup: {startup_warnings:?}");
        thread::sleep(Duration::from_millis(30));
    }
}

fn run(backend: BackendKind, root: &Path, nested: bool) {
    let (port, stop, server) = fixture(nested);
    let profiles = tempfile::tempdir().unwrap();
    let coordination = Arc::new(ManifestCoordination::default());
    let slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend,
            headless: true,
            profile_root: Some(profiles.path().join("profiles")),
            ..Default::default()
        },
        panel_local_id: "frame-test".into(),
        initial_url: Some(format!("http://127.0.0.1:{port}/")),
        width: 900,
        height: 700,
        frame_slot: slot.clone(),
        coordination: Some(coordination),
        capture_directory: None,
        video: Arc::default(),
        remote: None,
    })
    .unwrap();
    wait_for_first_frame(&session, &slot);
    let mut host = McpProcess::start(root);
    let readiness = host.action(BrowserControlAction::WaitForSelector {
        selector: "output[data-ready]".into(),
        state: horizon_browser::SelectorState::Present,
        timeout_millis: Some(15000),
    });
    if let Err(error) = readiness {
        let diagnostic = host.call("browser_evaluate", json!({"expression":"JSON.stringify({url:location.href,ready:document.readyState,text:document.body?.innerText,frames:[...document.querySelectorAll('iframe')].map(f=>f.src)})"}));
        let snapshot = host.call("browser_snapshot", json!({"max_nodes":100}));
        panic!("fixture readiness: {error}; document: {diagnostic:?}; snapshot: {snapshot:?}");
    }

    let snapshot = host.success(BrowserControlAction::Snapshot { max_nodes: 100 });
    let BrowserControlValue::Snapshot { snapshot } = snapshot else {
        panic!("snapshot")
    };
    let user = snapshot
        .nodes
        .iter()
        .find(|node| node.name == "User")
        .unwrap_or_else(|| panic!("frame input in snapshot: {:?}", snapshot.nodes));
    host.success(BrowserControlAction::Fill {
        target: BrowserTarget::Ref {
            reference: user.reference.clone(),
        },
        value: "synthetic-user".into(),
    });
    let password = host.nodes("input[type=password]");
    assert_eq!(password.len(), 1);
    host.success(BrowserControlAction::Fill {
        target: BrowserTarget::Ref {
            reference: password[0].reference.clone(),
        },
        value: "synthetic-secret".into(),
    });
    let submit = host.nodes("button");
    assert_eq!(submit.len(), 1);
    host.success(BrowserControlAction::Click {
        target: BrowserTarget::Ref {
            reference: submit[0].reference.clone(),
        },
        count: 1,
    });
    thread::sleep(Duration::from_millis(250));
    assert_eq!(host.nodes("output")[0].text, "Submitted");
    let top = host.success(BrowserControlAction::Evaluate {
        expression: "document.querySelector('#same').value === 'top-kept'".into(),
        timeout_millis: None,
    });
    assert!(matches!(top, BrowserControlValue::Json { value } if value==true));
    let old = host.nodes("input[type=password]")[0].reference.clone();
    host.success(BrowserControlAction::Evaluate {
        expression:
            "new Promise(resolve=>{const ready=e=>{if(e.data.ready){window.removeEventListener('message',ready);resolve(true)}};window.addEventListener('message',ready);document.querySelector('#widget').src=document.querySelector('#widget').src})"
                .into(),
        timeout_millis: None,
    });
    thread::sleep(Duration::from_millis(700));
    let stale = host.action(BrowserControlAction::Fill {
        target: BrowserTarget::Ref { reference: old },
        value: "must-not-fill".into(),
    });
    assert!(stale.unwrap_err().contains("stale_reference"));
    assert_eq!(host.nodes("input[type=password]").len(), 1);
    let audit = host.call("browser_audit", json!({})).unwrap().to_string();
    assert!(!audit.contains("synthetic-secret"));
    assert!(!audit.contains("synthetic-user"));
    drop(host);
    assert!(session.send(BrowserCommand::Stop));
    assert!(session.shutdown_signal().wait(Duration::from_secs(10)));
    stop.store(true, std::sync::atomic::Ordering::Release);
    server.join().unwrap();
}

fn fixture(nested: bool) -> (u16, Arc<std::sync::atomic::AtomicBool>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_stop = stop.clone();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        while !server_stop.load(std::sync::atomic::Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap_or(0);
            if size == 0 {
                continue;
            }
            let request = String::from_utf8_lossy(&request[..size]);
            let body = if request.starts_with("GET /form") {
                r#"<!doctype html><title>Frame form</title><form>
<label>User<input id="same" name="user" aria-label="User"></label><label>Password<input type="password" name="password" aria-label="Password"></label><button>Submit frame</button></form>
<script>top.postMessage({ready:true},'*');document.querySelector('form').onsubmit=e=>{e.preventDefault();top.postMessage({submitted:document.querySelector('[name=user]').value==='synthetic-user'&&document.querySelector('[name=password]').value==='synthetic-secret',trusted:e.isTrusted},'*')};</script>"#.to_owned()
            } else if request.starts_with("GET /wrapper") {
                format!(
                    r#"<!doctype html><title>Frame wrapper</title><iframe src="http://127.0.0.1:{port}/form" style="width:480px;height:220px"></iframe>"#
                )
            } else {
                let path = if nested { "wrapper" } else { "form" };
                format!(
                    r#"<!doctype html><title>Frame fixture</title><input id="same" value="top-kept"><output>Waiting</output><iframe id="widget" src="http://localhost:{port}/{path}" style="margin:50px;width:500px;height:250px"></iframe><script>window.addEventListener('message',e=>{{if(e.data.ready){{document.querySelector('output').setAttribute('data-ready','')}}else{{document.querySelector('output').textContent=e.data.submitted&&e.data.trusted?'Submitted':'Failed'}}}})</script>"#
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, stop, server)
}
