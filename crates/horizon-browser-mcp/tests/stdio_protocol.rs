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
        Self::start_as(home, "protocol-smoke")
    }

    fn start_as(home: &std::path::Path, actor: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-browser-mcp"))
            .env("HOME", home)
            .env("HORIZON_BROWSER_ROOT", home.join(".horizon"))
            .env("HORIZON_BROWSER_ACTOR", actor)
            .env("HORIZON_BROWSER_HOST_INSTANCE", "test-host")
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
fn stdio_negotiates_handshake_protocols_without_leaking_private_endpoints() {
    for (requested_version, negotiated_version) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
    ] {
        exercise_protocol(requested_version, negotiated_version);
    }
}

#[test]
fn discovery_tool_catalog_is_valid_for_per_request_protocol_clients() {
    let home = tempfile::tempdir().expect("isolated home");
    let mut process = McpProcess::start(home.path());
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let discovery = process.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": meta}
    }));
    assert!(discovery.get("error").is_none(), "{discovery}");
    let tools = process.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list",
        "params": {"_meta": meta}
    }));
    assert_eq!(tools["result"]["resultType"], "complete");
    assert_eq!(tools["result"]["ttlMs"], 0);
    assert_eq!(tools["result"]["cacheScope"], "private");
    assert_listed_tools_keep_the_browser_contract(&tools);
    let list = process.send(&json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"_meta": meta, "name": "browser_list", "arguments": {}}
    }));
    assert_eq!(list["result"]["isError"], false);
    assert_eq!(list["result"]["structuredContent"]["panels"], json!([]));
    process.close();
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
                && instructions.contains("browser_http_auth")
                && instructions.contains("browser_resize")
                && instructions.contains("browser_visibility")
                && instructions.contains("browser_close")
                && instructions.contains("allow_additional=true")
                && instructions.contains("one panel per engine")
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

fn assert_output_schemas_describe_objects(tools: &Value) {
    // Strict MCP clients reject tools/list when an outputSchema lacks a root type;
    // every advertised output schema must describe a JSON object.
    for tool in tools["result"]["tools"].as_array().expect("tools") {
        assert_eq!(
            tool["outputSchema"]["type"],
            json!("object"),
            "{} output schema must describe a JSON object: {}",
            tool["name"],
            tool["outputSchema"]
        );
    }
}

fn assert_input_schemas_describe_objects(tools: &Value) {
    // Model providers refuse a tool whose inputSchema has `oneOf`, `anyOf` or
    // `allOf` at the top level, and the client drops such a tool from the
    // agent's list. Every advertised input schema must describe one object.
    let mut seen = 0;
    for tool in tools["result"]["tools"].as_array().expect("tools") {
        seen += 1;
        let schema = &tool["inputSchema"];
        let keys = schema.as_object().map(|object| {
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            keys
        });
        assert_eq!(
            schema["type"],
            json!("object"),
            "{} input schema must describe a JSON object, keys {keys:?}: {schema}",
            tool["name"]
        );
        for combinator in ["oneOf", "anyOf", "allOf"] {
            assert!(
                schema.get(combinator).is_none_or(Value::is_null),
                "{} has a top-level {combinator}, which providers refuse, keys {keys:?}: {schema}",
                tool["name"]
            );
        }
    }
    assert!(seen > 0, "tools/list advertises tools");
}

fn assert_device_panel_contract(tools: &Value) {
    assert!(
        !listed_tool(tools, "device_panel")["inputSchema"]
            .to_string()
            .contains("browser_screenshot")
    );
    let device = listed_tool(tools, "device_panel").to_string();
    for field in ["identity", "machine_name", "hostname", "ip_addresses", "tailscale_name"] {
        assert!(device.contains(field), "missing Device identity field {field}");
    }
    for operation in ["create", "list", "inspect", "visibility", "reconnect", "close"] {
        assert!(
            device.contains(operation),
            "missing Device lifecycle operation {operation}"
        );
    }
}

fn assert_block_panel_contract(tools: &Value) {
    let card = listed_tool(tools, "block_panel").to_string();
    for operation in [
        "create",
        "update",
        "list",
        "inspect",
        "next_click",
        "visibility",
        "close",
        "open_terminal",
        "open_panel",
        "machine_key",
        "machine_keys",
        "delete_machine_key",
    ] {
        assert!(card.contains(operation), "missing card operation {operation}");
    }
    for key_field in [
        "remote_command",
        "preset",
        "directory",
        "ssh",
        "key",
        "public_key",
        "private_key_path",
        "known_hosts_path",
        "created_at_millis",
    ] {
        assert!(card.contains(key_field), "missing terminal or key field {key_field}");
    }
    for document_field in ["components", "heading", "buttons", "selection"] {
        assert!(card.contains(document_field), "missing document field {document_field}");
    }
    for click_field in [
        "event_id",
        "component_id",
        "revision",
        "selected",
        "at_millis",
        "stale",
        "timed_out",
        "pending_clicks",
        "listening",
    ] {
        assert!(card.contains(click_field), "missing click field {click_field}");
    }
    let description = listed_tool(tools, "block_panel")["description"]
        .as_str()
        .expect("description")
        .to_owned();
    for stated in [
        "not_owner",
        "too_many_cards",
        "no_cards",
        "Without panel_id",
        "selected true",
        "agent_present",
        "host_timeout",
        "at most once",
        "Its agent is not here",
        "Last seen: ",
        "A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.",
        "invalid_ssh_route",
        "invalid_remote_command",
        "too_many_terminals",
        "invalid_key_name",
        "too_many_keys",
        "key_unavailable",
        "IdentitiesOnly=yes",
        "accept-new",
    ] {
        assert!(description.contains(stated), "the description omits {stated}");
    }
    let next_click = listed_tool(tools, "block_panel")["inputSchema"].to_string();
    assert!(
        next_click.contains("timeout_millis"),
        "next_click takes its own wait: {next_click}"
    );
    assert!(
        next_click.contains("panel_id"),
        "next_click names a card when the caller wants one: {next_click}"
    );
}

fn assert_provider_tools_contract(tools: &Value) {
    let usage = listed_tool(tools, "browser_provider_usage");
    assert!(usage["inputSchema"]["properties"].get("provider").is_some());
    assert!(usage["inputSchema"]["properties"].get("credentials").is_none());
}

fn assert_orientation_contract(tools: &Value) {
    let rotation = listed_tool(tools, "browser_orientation");
    for field in ["panel_id", "orientation", "timeout_millis"] {
        assert!(rotation["inputSchema"]["properties"].get(field).is_some());
    }
    assert_eq!(
        rotation["inputSchema"]["$defs"]["Orientation"]["enum"],
        json!(["portrait", "landscape"])
    );
}

fn assert_screenshot_contract(tools: &Value) {
    let screenshot = listed_tool(tools, "browser_screenshot");
    for field in ["panel_id", "copy_to_clipboard"] {
        assert!(
            screenshot["inputSchema"]["properties"].get(field).is_some(),
            "screenshot schema lacks {field}"
        );
    }
    assert_eq!(
        screenshot["inputSchema"]["properties"]["copy_to_clipboard"]["default"],
        false
    );
}

fn assert_resize_contract(tools: &Value) {
    let resize = listed_tool(tools, "browser_resize");
    for field in ["panel_id", "width", "height", "reset", "timeout_millis"] {
        assert!(
            resize["inputSchema"]["properties"].get(field).is_some(),
            "resize schema lacks {field}"
        );
    }
    assert!(resize["description"].as_str().unwrap().contains("CSS pixels"));
    let zoom = listed_tool(tools, "browser_zoom");
    for field in ["panel_id", "percent", "reset", "timeout_millis"] {
        assert!(
            zoom["inputSchema"]["properties"].get(field).is_some(),
            "zoom schema lacks {field}"
        );
    }
    let description = zoom["description"].as_str().unwrap();
    assert!(description.contains("25-500") && description.contains("unsupported_backend"));
}

fn assert_download_fetch_contract(tools: &Value) {
    let downloads = listed_tool(tools, "browser_downloads");
    let arguments = downloads["inputSchema"]["properties"]
        .as_object()
        .expect("download arguments");
    assert!(arguments.contains_key("fetch"), "the answer can carry one file");
    assert!(
        downloads["description"]
            .as_str()
            .is_some_and(|description| description.contains("8 MiB")),
        "the description states the bound of the bytes that cross"
    );
}

fn assert_listed_tools_keep_the_browser_contract(tools: &Value) {
    let encoded_tools = tools.to_string();
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(28));
    assert_download_fetch_contract(tools);
    assert_output_schemas_describe_objects(tools);
    assert_input_schemas_describe_objects(tools);
    assert_catalog_contract(tools);
    assert_device_panel_contract(tools);
    assert_block_panel_contract(tools);
    assert_provider_tools_contract(tools);
    assert_orientation_contract(tools);
    assert_screenshot_contract(tools);
    assert_resize_contract(tools);
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
    assert!(
        create["description"]
            .as_str()
            .is_some_and(|description| description.contains("browser_list is empty")
                && description.contains("never create a helper panel")
                && description.contains("allow_additional=true")
                && description.contains("one panel per browser engine"))
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
    let http_auth = listed_tool(tools, "browser_http_auth");
    assert!(http_auth["description"].as_str().is_some_and(|description| {
        description.contains("Basic")
            && description.contains("Digest")
            && description.contains("password")
            && description.contains("If you encounter HTTP authentication")
            && description.contains("CLI run plans")
    }));
    assert!(http_auth["inputSchema"].to_string().contains("username"));
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
    assert_recovery_close_and_audit_contract(tools);
    assert!(!encoded_tools.contains("browser_ws"));
    assert!(!encoded_tools.contains("manifest_path"));
}

fn assert_recovery_close_and_audit_contract(tools: &Value) {
    let recovery = listed_tool(tools, "browser_remote_allocations");
    assert!(recovery["inputSchema"]["properties"].get("operation").is_some());
    assert!(recovery["inputSchema"]["properties"].get("reference").is_some());
    assert!(
        recovery["description"]
            .as_str()
            .is_some_and(|text| text.contains("exact-session absence"))
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
}

#[test]
fn resize_preserves_scope_user_control_and_measured_result_contract() {
    use horizon_browser_control::manifest::{self, BrowserManifest, ManifestOwner, ManifestWorkspace};
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".horizon");
    let path = manifest::manifest_path_for_root(&root, "panel");
    let actor = "horizon:resize-test";
    let baseline = BrowserManifest {
        panel_local_id: "panel".into(),
        browser_ws: "private-endpoint".into(),
        host: Some("test-host".into()),
        workspace: Some(ManifestWorkspace {
            host_instance: "test-host".into(),
            local_id: "workspace".into(),
            actors: vec![actor.into()],
        }),
        updated_at: manifest::now_millis(),
        ..BrowserManifest::default()
    };
    let mut process = McpProcess::start_as(home.path(), actor);
    process.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}));
    process.notify(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    for scenario in ["workspace", "owner", "user", "handoff"] {
        let mut panel = baseline.clone();
        match scenario {
            "workspace" => panel.workspace.as_mut().unwrap().actors.clear(),
            "owner" => {
                panel.owner = Some(ManifestOwner {
                    name: "other".into(),
                    tty: None,
                    updated_at: manifest::now_millis(),
                });
            }
            "user" => {
                panel.user_active = true;
                panel.user_active_at = manifest::now_millis();
            }
            "handoff" => {
                panel.handoff = Some(manifest::ManifestHandoff {
                    request_id: "handoff".into(),
                    reason: "user turn".into(),
                    requested_at: manifest::now_millis(),
                    done: false,
                });
            }
            _ => unreachable!(),
        }
        manifest::write_at(&path, &panel).unwrap();
        let result=process.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"browser_resize","arguments":{"panel_id":"panel","width":390,"height":844}}}));
        assert_eq!(result["result"]["isError"], true, "{scenario}: {result}");
        assert!(manifest::read_at(&path).unwrap().actions.is_empty());
    }
    for failure in [
        None,
        Some("viewport_failed"),
        Some("remote_viewport_fixed"),
        Some("viewport_unsupported"),
    ] {
        manifest::write_at(&path, &baseline).unwrap();
        let worker_path = path.clone();
        let worker_root = root.clone();
        let worker = resize_result_fixture(worker_path, worker_root, failure);
        let result=process.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"browser_resize","arguments":{"panel_id":"panel","width":390,"height":844}}}));
        worker.join().unwrap();
        if let Some(code) = failure {
            assert_eq!(result["result"]["isError"], true);
            assert!(result.to_string().contains(code), "expected {code}: {result}");
        } else {
            assert_eq!(
                result["result"]["structuredContent"]["applied"],
                json!({"width":390,"height":844})
            );
        }
    }
    process.close();
}

fn resize_result_fixture(
    worker_path: std::path::PathBuf,
    worker_root: std::path::PathBuf,
    failure: Option<&'static str>,
) -> std::thread::JoinHandle<()> {
    use horizon_browser_control::manifest;
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(action) = manifest::read_at(&worker_path).and_then(|panel| panel.actions.into_iter().next()) {
                assert!(matches!(
                    action.action,
                    horizon_browser::BrowserControlAction::Resize {
                        viewport: Some([390, 844]),
                        ..
                    }
                ));
                let result = match failure {
                    Some(code) => horizon_browser::AgentActionResult::failed(
                        action.action_id.clone(),
                        horizon_browser::BrowserControlFailure::new(code, "fixture refusal"),
                    ),
                    None => horizon_browser::AgentActionResult::completed(
                        action.action_id.clone(),
                        horizon_browser::BrowserControlValue::Viewport {
                            requested: Some([390, 844]),
                            applied: [390, 844],
                            zoom_percent: 100,
                        },
                    ),
                };
                let result_path = manifest::action_result_path_for_root(&worker_root, "panel", &action.action_id);
                std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
                let mut temporary = tempfile::NamedTempFile::new_in(result_path.parent().unwrap()).unwrap();
                serde_json::to_writer(temporary.as_file_mut(), &result).unwrap();
                temporary.persist(result_path).unwrap();
                break;
            }
            assert!(std::time::Instant::now() < deadline, "resize was not enqueued");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    })
}

fn assert_catalog_contract(tools: &Value) {
    let devices = listed_tool(tools, "browser_provider_devices");
    assert!(devices["inputSchema"]["properties"]["provider"].is_object());
    assert!(devices["inputSchema"]["properties"].get("credentials").is_none());
}

#[test]
fn screenshot_refuses_standalone_without_claiming_browser() {
    use horizon_browser_control::manifest::{self, BrowserManifest};
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".horizon");
    let path = manifest::manifest_path_for_root(&root, "panel");
    manifest::write_at(
        &path,
        &BrowserManifest {
            panel_local_id: "panel".into(),
            updated_at: manifest::now_millis(),
            ..BrowserManifest::default()
        },
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut process = McpProcess::start(home.path());
    process.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}));
    process.notify(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let result = process.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"browser_screenshot","arguments":{"panel_id":"panel"}}}));
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(manifest::read_at(&path).unwrap().owner.is_none());
    process.close();
}
