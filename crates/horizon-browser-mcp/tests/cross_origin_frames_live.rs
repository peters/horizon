//! Cross-origin form acceptance through the public MCP stdio tools.
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

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires local Safari automation and loopback sockets"]
fn mcp_safari_preserves_top_level_input_and_frame_boundaries() {
    let root = tempfile::tempdir().unwrap();
    horizon_browser_control::paths::configure_runtime_root(root.path()).unwrap();
    run(BackendKind::SafariWebDriver, root.path(), true);
}

fn safari_compatibility(host: &mut McpProcess) {
    let snapshot = host.call("browser_snapshot", json!({"max_nodes":100})).unwrap();
    assert!(
        snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["role"] == "iframe")
    );
    assert!(host.nodes("form input[type=password]").is_empty());
    let top = host.nodes("#same");
    assert_eq!(top.len(), 1);
    host.call(
        "browser_act",
        json!({"action":"fill","ref":top[0].reference,"value":"safari-synthetic"}),
    )
    .unwrap();
    let value = host
        .call(
            "browser_evaluate",
            json!({"expression":"document.querySelector('#same').value"}),
        )
        .unwrap();
    assert_eq!(value["value"], "safari-synthetic");
    protected_controls(host, false);
    unfocused_controls(host, false);
    queued_controls(host, false);
    text_controls(host, false);
    assert!(
        !host
            .call("browser_audit", json!({}))
            .unwrap()
            .to_string()
            .contains("safari-synthetic")
    );
}

fn protected_controls(host: &mut McpProcess, child: bool) {
    let prefix = if child { "" } else { "top-" };
    let cases = [
        ("protected-input", "input-kept", "0", "element_not_editable"),
        ("protected-textarea", "textarea-kept", "0", "element_not_editable"),
        ("protected-aria", "aria-kept", "0", "element_not_editable"),
        ("protected-editor", "editor-kept", "0", "element_not_editable"),
        ("protected-onfocus", "focus-kept", "0", "element_not_editable"),
        ("disabled-focus-native", "disabled-kept", "0", "element_disabled"),
        ("disabled-focus-aria", "disabled-kept", "0", "element_disabled"),
        ("disabled-input-native", "", "1", "element_disabled"),
        ("disabled-input-aria", "", "1", "element_disabled"),
        ("disabled-queued-focus-native", "disabled-kept", "0", "element_disabled"),
        ("disabled-queued-focus-aria", "disabled-kept", "0", "element_disabled"),
        ("disabled-queued-input-native", "", "1", "element_disabled"),
        ("disabled-queued-input-aria", "", "1", "element_disabled"),
    ];
    let selector = cases.map(|(id, ..)| format!("#{prefix}{id}")).join(",");
    let fields = host.nodes(&selector);
    assert_eq!(fields.len(), cases.len());
    let errors: Vec<_> = fields
        .into_iter()
        .map(|field| {
            host.call(
                "browser_act",
                json!({"action":"fill","ref":field.reference,"value":"rejected"}),
            )
            .err()
            .unwrap_or_default()
        })
        .collect();
    let expression = if child {
        let inspect = host.nodes("#inspect-protected");
        host.call("browser_act", json!({"action":"click","ref":inspect[0].reference}))
            .unwrap();
        host.call(
            "browser_wait",
            json!({"selector":"output[data-protected]","state":"present","timeout_millis":2000}),
        )
        .unwrap();
        "window.protectedValues".to_owned()
    } else {
        format!(
            "[...document.querySelectorAll({})].map(e=>[e.value??e.textContent,e.dataset.inputs??'0'])",
            serde_json::to_string(&selector).unwrap()
        )
    };
    let values = host.call("browser_evaluate", json!({"expression":expression})).unwrap();
    assert_eq!(
        values["value"],
        json!(cases.map(|(_, value, inputs, _)| [value, inputs]))
    );
    assert!(
        errors
            .iter()
            .zip(cases)
            .all(|(error, (_, _, _, code))| error.contains(code))
    );
}

fn unfocused_controls(host: &mut McpProcess, child: bool) {
    let prefix = if child { "" } else { "top-" };
    let selector = format!("#{prefix}focus-redirect,#{prefix}inert-input,#{prefix}focus-oninput");
    let fields = host.nodes(&selector);
    assert_eq!(fields.len(), 3);
    for field in fields {
        assert!(
            host.call(
                "browser_act",
                json!({"action":"fill","ref":field.reference,"value":"rejected"}),
            )
            .unwrap_err()
            .contains("element_not_focused")
        );
    }
    let selector =
        format!("#{prefix}focus-redirect,#{prefix}inert-input,.{prefix}focus-oninput,.{prefix}input-recipient");
    let expression = if child {
        let inspect = host.nodes("#inspect-unfocused");
        host.call("browser_act", json!({"action":"click","ref":inspect[0].reference}))
            .unwrap();
        host.call(
            "browser_wait",
            json!({"selector":"output[data-unfocused]","state":"present","timeout_millis":2000}),
        )
        .unwrap();
        "window.unfocusedValues".to_owned()
    } else {
        format!(
            "[...document.querySelectorAll({})].map(e=>[e.value,e.dataset.inputs??'0'])",
            serde_json::to_string(&selector).unwrap()
        )
    };
    let values = host.call("browser_evaluate", json!({"expression":expression})).unwrap();
    assert_eq!(
        values["value"],
        json!([
            ["redirect-kept", "0"],
            ["inert-kept", "0"],
            ["", "1"],
            ["recipient-kept", "0"]
        ])
    );
}

fn queued_controls(host: &mut McpProcess, child: bool) {
    let prefix = if child { "" } else { "top-" };
    let fields = host.nodes(&format!(".{prefix}queued-control"));
    assert_eq!(fields.len(), 2);
    let mut errors = Vec::new();
    for field in fields {
        errors.push(
            host.call(
                "browser_act",
                json!({"action":"fill","ref":field.reference,"value":"rejected"}),
            )
            .err()
            .unwrap_or_default(),
        );
    }
    let expression = if child {
        let inspect = host.nodes("#inspect-queued");
        host.call("browser_act", json!({"action":"click","ref":inspect[0].reference}))
            .unwrap();
        host.call(
            "browser_wait",
            json!({"selector":"output[data-queued]","state":"present","timeout_millis":2000}),
        )
        .unwrap();
        "window.queuedValues".to_owned()
    } else {
        format!(
            "[...document.querySelectorAll('.{prefix}queued-control'),document.querySelector('.{prefix}input-recipient')].map(e=>[e.value,e.dataset.inputs??'0'])"
        )
    };
    let values = host.call("browser_evaluate", json!({"expression":expression})).unwrap();
    assert_eq!(
        values["value"],
        json!([["queued-focus-kept", "0"], ["", "1"], ["recipient-kept", "0"]])
    );
    assert!(errors.iter().all(|error| error.contains("element_not_focused")));
}

fn text_controls(host: &mut McpProcess, child: bool) {
    let prefix = if child { "" } else { "top-" };
    let fields = host.nodes(&format!(".{prefix}unsupported-control"));
    assert_eq!(fields.len(), 15);
    for field in fields {
        assert!(
            host.call(
                "browser_act",
                json!({"action":"fill","ref":field.reference,"value":"rejected"})
            )
            .unwrap_err()
            .contains("element_not_editable")
        );
    }
    let fields = host.nodes(&format!(".{prefix}supported-control"));
    assert_eq!(fields.len(), 9);
    let expected = [
        "typed-synthetic",
        "typed-synthetic",
        "typed-synthetic",
        "typed-synthetic",
        "typed-synthetic",
        "typed-synthetic",
        "314159265",
        "typed-synthetic",
        "typed-synthetic",
    ];
    for (field, value) in fields.iter().zip(expected) {
        host.call(
            "browser_act",
            json!({"action":"fill","ref":field.reference,"value":value}),
        )
        .unwrap();
    }
    let expression = if child {
        let inspect = host.nodes("#inspect-controls");
        host.call("browser_act", json!({"action":"click","ref":inspect[0].reference}))
            .unwrap();
        host.call(
            "browser_wait",
            json!({"selector":"output[data-controls]","state":"present","timeout_millis":2000}),
        )
        .unwrap();
        "window.controlValues".to_owned()
    } else {
        "({unsupported:[...document.querySelectorAll('.top-unsupported-control')].map(e=>[e.value,e.dataset.initial,e.dataset.inputs]),supported:[...document.querySelectorAll('.top-supported-control')].map(e=>e.value??e.textContent)})".to_owned()
    };
    let values = host.call("browser_evaluate", json!({"expression":expression})).unwrap();
    let values = &values["value"];
    let unsupported = values["unsupported"].as_array().unwrap();
    assert_eq!(unsupported.len(), 15);
    for field in unsupported {
        assert_eq!(field[0], field[1]);
        assert_eq!(field[2], "0");
    }
    assert_eq!(values["supported"], json!(expected));
    let audit = host.call("browser_audit", json!({})).unwrap().to_string();
    assert!(!audit.contains("typed-synthetic") && !audit.contains("314159265"));
}

fn control_fixture(prefix: &str) -> String {
    let unsupported = [
        "file",
        "checkbox",
        "radio",
        "range",
        "button",
        "submit",
        "reset",
        "color",
        "date",
        "time",
        "datetime-local",
        "month",
        "week",
    ]
    .map(|kind| {
        format!("<input class='{prefix}unsupported-control' contenteditable='true' type='{kind}' value='kept'>")
    })
    .join("");
    let supported = ["text", "search", "tel", "url", "email", "password", "number"]
        .map(|kind| {
            format!("<input class='{prefix}supported-control' contenteditable='true' type='{kind}' value='old-kept'>")
        })
        .join("");
    format!(
        r#"{unsupported}<input class="{prefix}unsupported-control" type="text" value="kept" onfocus="this.type='checkbox'"><select class="{prefix}unsupported-control" contenteditable="true"><option value="kept">Kept</option></select>{supported}<textarea class="{prefix}supported-control" contenteditable="true">old-kept</textarea><div class="{prefix}supported-control" contenteditable="true">old-kept</div><button id="{prefix}inspect-controls" type="button">Inspect controls</button><script>document.querySelectorAll('[id^="{prefix}protected-"]').forEach(e=>e.addEventListener('input',()=>{{e.dataset.inputs=String(Number(e.dataset.inputs||0)+1)}}));document.querySelectorAll('.{prefix}unsupported-control').forEach(e=>{{e.dataset.initial=e.value;e.dataset.inputs='0';e.oninput=()=>e.dataset.inputs=String(Number(e.dataset.inputs)+1)}});document.querySelector('#{prefix}inspect-controls').onclick=()=>parent.postMessage({{controls:{{unsupported:[...document.querySelectorAll('.{prefix}unsupported-control')].map(e=>[e.value,e.dataset.initial,e.dataset.inputs]),supported:[...document.querySelectorAll('.{prefix}supported-control')].map(e=>e.value??e.textContent)}}}},'*')</script>"#
    ) + &queued_fixture(prefix)
        + &disabled_fixture(prefix)
}

fn disabled_fixture(prefix: &str) -> String {
    [
        ("disabled-focus-native", "this.disabled=true", ""),
        ("disabled-focus-aria", "this.setAttribute('aria-disabled','true')", ""),
        ("disabled-input-native", "", "this.disabled=true"),
        ("disabled-input-aria", "", "this.setAttribute('aria-disabled','true')"),
        ("disabled-queued-focus-native", "queueMicrotask(()=>this.disabled=true)", ""),
        ("disabled-queued-focus-aria", "queueMicrotask(()=>this.setAttribute('aria-disabled','true'))", ""),
        ("disabled-queued-input-native", "", "queueMicrotask(()=>queueMicrotask(()=>this.disabled=true))"),
        ("disabled-queued-input-aria", "", "queueMicrotask(()=>queueMicrotask(()=>this.setAttribute('aria-disabled','true')))"),
    ].map(|(id, focus, input)| format!(r#"<input id="{prefix}{id}" value="disabled-kept" onfocus="{focus}" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1);{input}">"#)).join("")
}

fn queued_fixture(prefix: &str) -> String {
    format!(
        r#"<input class="{prefix}queued-control" id="{prefix}queued-onfocus" value="queued-focus-kept" onfocus="queueMicrotask(()=>document.querySelector('.{prefix}input-recipient').focus())" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)">
<input class="{prefix}queued-control" id="{prefix}queued-oninput" value="queued-input-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1);queueMicrotask(()=>queueMicrotask(()=>{{const recipient=document.querySelector('.{prefix}input-recipient');recipient.id=this.id;this.removeAttribute('id');recipient.focus()}}))">
<button id="{prefix}inspect-queued" type="button" onclick="parent.postMessage({{queued:[...document.querySelectorAll('.{prefix}queued-control'),document.querySelector('.{prefix}input-recipient')].map(e=>[e.value,e.dataset.inputs??'0'])}},'*')">Inspect queued fields</button>"#
    )
}

fn edge_cases(host: &mut McpProcess) {
    assert!(
        host.call("browser_query", json!({"selector":"["}))
            .unwrap_err()
            .contains("invalid_selector")
    );
    let capped = host
        .call("browser_query", json!({"selector":"input","max_results":1}))
        .unwrap();
    assert_eq!(capped["nodes"].as_array().unwrap().len(), 1);
    assert!(!capped["nodes"][0]["bounds"].is_null());
    for (selector, code) in [
        ("#disabled", "element_disabled"),
        ("#hidden", "element_not_visible"),
        ("#readonly", "element_not_editable"),
    ] {
        let nodes = host.nodes(selector);
        assert_eq!(nodes.len(), 1);
        assert!(
            host.call(
                "browser_act",
                json!({"action":"fill","ref":nodes[0].reference,"value":"rejected"})
            )
            .unwrap_err()
            .contains(code)
        );
    }
    protected_controls(host, true);
    unfocused_controls(host, true);
    queued_controls(host, true);
    text_controls(host, true);
    let files = host.nodes(".unsupported-control[type=file]");
    assert_eq!(files.len(), 1);
    assert!(files[0].file_input.is_none());
    assert!(
        host.call(
            "browser_act",
            json!({"action":"scroll","ref":files[0].reference,"delta_y":1})
        )
        .unwrap_err()
        .contains("unsupported_frame_action")
    );
    host.call("browser_evaluate", json!({"expression":"new Promise(resolve=>{const ready=e=>{if(e.data.ready){window.removeEventListener('message',ready);resolve(true)}};window.addEventListener('message',ready);const frame=document.createElement('iframe');frame.id='sibling';frame.src=document.querySelector('#widget').src;document.body.append(frame)})"})).unwrap();
    let capped = host
        .call(
            "browser_query",
            json!({"selector":"form input[type=password]","max_results":1}),
        )
        .unwrap();
    assert_eq!(capped["nodes"].as_array().unwrap().len(), 1);
    let fields = host.nodes("form input[type=password]");
    assert_eq!(fields.len(), 2, "same-URL sibling documents must have distinct refs");
    for (field, value) in fields.iter().zip(["edge-a", "edge-b"]) {
        host.call(
            "browser_act",
            json!({"action":"fill","ref":field.reference,"value":value}),
        )
        .unwrap();
    }
    let values = host
        .call(
            "browser_evaluate",
            json!({"expression":"Object.values(window.states).filter(s=>s.value==='edge-a'||s.value==='edge-b')"}),
        )
        .unwrap();
    let values = values["value"].as_array().unwrap();
    assert_eq!(values.len(), 2, "each ref must fill a different document");
    let removed = values.iter().find(|state| state["owner"] == "sibling").unwrap();
    let index = usize::from(removed["value"] == "edge-b");
    host.call(
        "browser_evaluate",
        json!({"expression":"(()=>{document.querySelector('#sibling').remove();return true})()"}),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(250));
    assert!(
        host.call(
            "browser_act",
            json!({"action":"fill","ref":fields[index].reference,"value":"rejected"})
        )
        .unwrap_err()
        .contains("stale_reference")
    );
    host.call(
        "browser_act",
        json!({"action":"fill","ref":fields[1-index].reference,"value":"survivor"}),
    )
    .unwrap();
    assert_eq!(host.nodes("form input[type=password]").len(), 1);
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

fn check_frame_delivery(host: &mut McpProcess, slot: &FrameSlot) {
    let frame_seq = slot.latest().unwrap().seq;
    host.call(
        "browser_evaluate",
        json!({"expression":"document.body.style.backgroundColor='rgb(12, 34, 56)'"}),
    )
    .unwrap();
    let capture = host.nodes("body > #same");
    assert_eq!(capture.len(), 1);
    host.call(
        "browser_act",
        json!({"action":"fill","ref":capture[0].reference,"value":"capture-synthetic"}),
    )
    .unwrap();
    assert!(
        !host
            .call("browser_audit", json!({}))
            .unwrap()
            .to_string()
            .contains("capture-synthetic")
    );
    let frame_deadline = Instant::now() + Duration::from_secs(3);
    while slot.latest().unwrap().seq == frame_seq {
        assert!(Instant::now() < frame_deadline, "frame delivery stopped during input");
        thread::sleep(Duration::from_millis(30));
    }
}

fn run(backend: BackendKind, root: &Path, nested: bool) {
    let (port, stop, server) = fixture(nested);
    let profiles = tempfile::Builder::new().prefix("frame-profiles-").tempdir().unwrap();
    let coordination = Arc::new(ManifestCoordination::default());
    let slot = Arc::new(FrameSlot::new());
    let session = start_session(BrowserSessionConfig {
        browser: BrowserConfig {
            backend,
            headless: backend != BackendKind::SafariWebDriver,
            profile_root: (backend != BackendKind::SafariWebDriver).then(|| profiles.path().join("profiles")),
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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

        if backend == BackendKind::SafariWebDriver {
            safari_compatibility(&mut host);
        } else {
            form_roundtrip(&mut host);
        }
        check_frame_delivery(&mut host, &slot);
        drop(host);
    }));
    assert!(session.send(BrowserCommand::Stop));
    assert!(session.shutdown_signal().wait(Duration::from_secs(10)));
    stop.store(true, std::sync::atomic::Ordering::Release);
    server.join().unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

fn form_roundtrip(host: &mut McpProcess) {
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
    let password = host.nodes("form input[type=password]");
    assert_eq!(password.len(), 1);
    assert!(
        host.call(
            "browser_act",
            json!({"action":"fill","ref":user.reference,"value":"must-not-fill"})
        )
        .unwrap_err()
        .contains("stale_reference")
    );
    host.success(BrowserControlAction::Fill {
        target: BrowserTarget::Ref {
            reference: password[0].reference.clone(),
        },
        value: "synthetic-secret".into(),
    });
    let submit = host.nodes("button[type=submit]");
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
    for _ in 0..3 {
        let old = host.nodes("form input[type=password]")[0].reference.clone();
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
        assert_eq!(host.nodes("form input[type=password]").len(), 1);
    }
    edge_cases(host);
    protected_controls(host, false);
    unfocused_controls(host, false);
    queued_controls(host, false);
    text_controls(host, false);
    let audit = host.call("browser_audit", json!({})).unwrap().to_string();
    for secret in [
        "synthetic-secret",
        "synthetic-user",
        "edge-a",
        "edge-b",
        "survivor",
        "rejected",
        "must-not-fill",
    ] {
        assert!(!audit.contains(secret));
    }
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
<label>User<input id="same" name="user" aria-label="User"></label><label>Password<input type="password" name="password" aria-label="Password"></label><button type="submit">Submit frame</button></form><input id="disabled" disabled><input id="hidden" style="display:none"><div id="readonly">Read only</div><input id="protected-input" readonly value="input-kept"><textarea id="protected-textarea" readonly>textarea-kept</textarea><input id="protected-aria" aria-readonly="true" value="aria-kept"><div id="protected-editor" contenteditable="true" aria-readonly="true">editor-kept</div><input id="protected-onfocus" value="focus-kept" onfocus="this.readOnly=true"><input id="focus-redirect" value="redirect-kept" onfocus="document.querySelector('#same').focus()" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"><div inert><input id="inert-input" value="inert-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"></div><input id="focus-oninput" class="focus-oninput" value="input-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1);const recipient=document.querySelector('.input-recipient');recipient.id=this.id;this.removeAttribute('id');recipient.focus()"><input id="input-recipient" class="input-recipient" value="recipient-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"><button id="inspect-unfocused" type="button">Inspect unfocused fields</button><button id="inspect-protected" type="button">Inspect protected fields</button>
<script>document.querySelector('#inspect-unfocused').onclick=()=>parent.postMessage({unfocused:[...document.querySelectorAll('#focus-redirect,#inert-input,.focus-oninput,.input-recipient')].map(e=>[e.value,e.dataset.inputs??'0'])},'*');document.querySelector('#inspect-protected').onclick=()=>parent.postMessage({protected:[...document.querySelectorAll('#protected-input,#protected-textarea,#protected-aria,#protected-editor,#protected-onfocus,[id^=disabled-]')].map(e=>[e.value??e.textContent,e.dataset.inputs??'0'])},'*');const token=crypto.randomUUID();parent.postMessage({ready:true},'*');document.querySelector('form').oninput=()=>parent.postMessage({state:token,value:document.querySelector('[name=password]').value},'*');document.querySelector('form').onsubmit=e=>{e.preventDefault();parent.postMessage({submitted:document.querySelector('[name=user]').value==='synthetic-user'&&document.querySelector('[name=password]').value==='synthetic-secret',trusted:e.isTrusted},'*')};</script>"#.to_owned() + &control_fixture("")
            } else if request.starts_with("GET /wrapper") {
                format!(
                    r#"<!doctype html><title>Frame wrapper</title><iframe src="http://127.0.0.1:{port}/form" style="width:480px;height:220px"></iframe><script>window.addEventListener('message',e=>top.postMessage(e.data,'*'))</script>"#
                )
            } else {
                let path = if nested { "wrapper" } else { "form" };
                let controls = control_fixture("top-");
                format!(
                    r#"<!doctype html><title>Frame fixture</title><input id="same" value="top-kept"><input id="top-protected-input" readonly value="input-kept"><textarea id="top-protected-textarea" readonly>textarea-kept</textarea><input id="top-protected-aria" aria-readonly="true" value="aria-kept"><div id="top-protected-editor" contenteditable="true" aria-readonly="true">editor-kept</div><input id="top-protected-onfocus" value="focus-kept" onfocus="this.readOnly=true"><input id="top-focus-redirect" value="redirect-kept" onfocus="document.querySelector('#same').focus()" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"><div inert><input id="top-inert-input" value="inert-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"></div><input id="top-focus-oninput" class="top-focus-oninput" value="input-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1);const recipient=document.querySelector('.top-input-recipient');recipient.id=this.id;this.removeAttribute('id');recipient.focus()"><input id="top-input-recipient" class="top-input-recipient" value="recipient-kept" oninput="this.dataset.inputs=String(Number(this.dataset.inputs||0)+1)"><output>Waiting</output><iframe id="widget" src="http://localhost:{port}/{path}" style="margin:50px;width:500px;height:250px"></iframe><script>window.states={{}};window.addEventListener('message',e=>{{if(e.data.state){{window.states[e.data.state]={{owner:e.source===document.querySelector('#widget').contentWindow?'widget':'sibling',value:e.data.value}}}}else if(e.data.controls){{window.controlValues=e.data.controls;document.querySelector('output').setAttribute('data-controls','')}}else if(e.data.queued){{window.queuedValues=e.data.queued;document.querySelector('output').setAttribute('data-queued','')}}else if(e.data.unfocused){{window.unfocusedValues=e.data.unfocused;document.querySelector('output').setAttribute('data-unfocused','')}}else if(e.data.protected){{window.protectedValues=e.data.protected;document.querySelector('output').setAttribute('data-protected','')}}else if(e.data.ready){{document.querySelector('output').setAttribute('data-ready','')}}else{{document.querySelector('output').textContent=e.data.submitted&&e.data.trusted?'Submitted':'Failed'}}}})</script>{controls}"#
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
