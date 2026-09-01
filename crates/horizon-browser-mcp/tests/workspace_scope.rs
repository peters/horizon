//! Workspace scoping of MCP discovery and control, end to end over stdio.
//!
//! The Horizon host stamps every live browser manifest with the agent
//! identities sharing the panel's workspace. These tests seed manifests the
//! way the host would and prove that a Horizon agent identity can only see
//! and control the panels stamped with it, that the stamp is re-read on every
//! call (so panel moves apply without restarting the server), and that
//! identities from outside Horizon keep the unscoped behavior. Stamps also
//! name the host process, so a second Horizon process running a duplicated
//! session (same persisted agent id, different host instance) is refused.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use horizon_core::browser::manifest::{
    BrowserManifest, ManifestOwner, ManifestWorkspace, manifest_path_for_root, read_at, write_at,
};
use serde_json::{Value, json};

const AGENT_A: &str = "horizon:agent-a";
const HOST_A: &str = "host-a";
const HOST_B: &str = "host-b";
const SAME_WORKSPACE_PANEL: &str = "same-workspace";
const OTHER_WORKSPACE_PANEL: &str = "other-workspace";
const OTHER_HOST_PANEL: &str = "other-host";
const STALE_STAMP_PANEL: &str = "stale-stamp";
const LEGACY_PANEL: &str = "legacy";

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpProcess {
    fn start(home: &Path, actor: &str, host_instance: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser-mcp"));
        command
            .env("HOME", home)
            .env("HORIZON_BROWSER_ACTOR", actor)
            .env_remove("HORIZON_BROWSER_HOST_INSTANCE")
            .env("RUST_LOG", "off");
        if let Some(host_instance) = host_instance {
            command.env("HORIZON_BROWSER_HOST_INSTANCE", host_instance);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn MCP server");
        let stdin = child.stdin.take().expect("MCP stdin");
        let stdout = BufReader::new(child.stdout.take().expect("MCP stdout"));
        let mut process = Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
        };
        let initialize = process.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "horizon-test", "version": "1" }
            }),
        );
        assert_eq!(initialize["result"]["serverInfo"]["name"], "horizon-browser");
        assert!(
            initialize["result"]["instructions"]
                .as_str()
                .is_some_and(|instructions| instructions.contains("scoped to the workspace")
                    && instructions.contains("visible field is host presentation state")),
            "server instructions must teach workspace scoping"
        );
        process.write(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        process
    }

    fn write(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("open MCP stdin");
        serde_json::to_writer(&mut *stdin, message).expect("encode MCP message");
        stdin.write_all(b"\n").expect("terminate MCP message");
        stdin.flush().expect("flush MCP message");
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut response = String::new();
        self.stdout.read_line(&mut response).expect("read MCP response");
        assert!(!response.is_empty(), "MCP server closed without a response");
        let response: Value = serde_json::from_str(&response).expect("decode MCP response");
        assert_eq!(response["id"], id);
        response
    }

    fn call(&mut self, tool: &str, arguments: &Value) -> Value {
        let response = self.request("tools/call", &json!({ "name": tool, "arguments": arguments }));
        response["result"].clone()
    }

    fn listed_panel_ids(&mut self) -> Vec<String> {
        let list = self.call("browser_list", &json!({}));
        assert_eq!(list["isError"], false, "browser_list failed: {list}");
        list["structuredContent"]["panels"]
            .as_array()
            .expect("panel list")
            .iter()
            .map(|panel| panel["panel_id"].as_str().expect("panel id").to_string())
            .collect()
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

fn horizon_root(home: &Path) -> PathBuf {
    home.join(".horizon")
}

/// A manifest as its driver writes it: `host` names the process running the
/// browser, and the host's sync stamps `workspace` with the same host.
fn manifest(
    panel_local_id: &str,
    workspace: Option<ManifestWorkspace>,
    owner: Option<ManifestOwner>,
) -> BrowserManifest {
    let host = workspace.as_ref().map(|workspace| workspace.host_instance.clone());
    manifest_with_host(panel_local_id, host, workspace, owner)
}

fn manifest_with_host(
    panel_local_id: &str,
    host: Option<String>,
    workspace: Option<ManifestWorkspace>,
    owner: Option<ManifestOwner>,
) -> BrowserManifest {
    BrowserManifest {
        panel_local_id: panel_local_id.to_string(),
        browser_ws: "ws://127.0.0.1:1/devtools/browser/private".to_string(),
        target_id: "target".to_string(),
        url: "https://example.test/".to_string(),
        title: panel_local_id.to_string(),
        workspace,
        host,
        owner,
        ..BrowserManifest::default()
    }
}

fn write_manifest(home: &Path, manifest: &BrowserManifest) {
    write_at(
        &manifest_path_for_root(&horizon_root(home), &manifest.panel_local_id),
        manifest,
    )
    .expect("write manifest");
}

fn read_manifest(home: &Path, panel_local_id: &str) -> BrowserManifest {
    read_at(&manifest_path_for_root(&horizon_root(home), panel_local_id)).expect("read manifest")
}

fn stale_owner(actor: &str) -> ManifestOwner {
    ManifestOwner {
        name: actor.to_string(),
        tty: None,
        updated_at: 1,
    }
}

/// Seed the manifests one Horizon home can hold at the same time: a panel in
/// the caller's workspace, an unowned panel with a stale owner in another
/// workspace, a panel stamped by a second live host for a duplicated session
/// that reuses the caller's persisted agent id and workspace local id, a
/// panel whose driver restarted under another host while the caller's stamp
/// was still on it, and a legacy manifest from a host that never stamped
/// workspaces.
fn seed_home(home: &Path) {
    write_manifest(
        home,
        &manifest(
            SAME_WORKSPACE_PANEL,
            Some(ManifestWorkspace::new(
                HOST_A,
                "workspace-a",
                vec![AGENT_A.to_string(), "horizon:agent-c".to_string()],
            )),
            None,
        ),
    );
    write_manifest(
        home,
        &manifest(
            OTHER_WORKSPACE_PANEL,
            Some(ManifestWorkspace::new(
                HOST_A,
                "workspace-b",
                vec!["horizon:agent-b".to_string()],
            )),
            Some(stale_owner("horizon:agent-b")),
        ),
    );
    write_manifest(
        home,
        &manifest(
            OTHER_HOST_PANEL,
            Some(ManifestWorkspace::new(HOST_B, "workspace-a", vec![AGENT_A.to_string()])),
            None,
        ),
    );
    write_manifest(
        home,
        &manifest_with_host(
            STALE_STAMP_PANEL,
            Some(HOST_B.to_string()),
            Some(ManifestWorkspace::new(HOST_A, "workspace-a", vec![AGENT_A.to_string()])),
            None,
        ),
    );
    write_manifest(home, &manifest(LEGACY_PANEL, None, None));
}

fn assert_outside_workspace(result: &Value, panel_id: &str) {
    assert_eq!(result["isError"], true, "expected rejection for {panel_id}: {result}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains(&format!(
            "browser panel {panel_id} is outside the calling agent's Horizon workspace"
        )),
        "unexpected rejection for {panel_id}: {text}"
    );
}

#[test]
fn horizon_agents_only_see_and_control_their_own_workspace() {
    let home = tempfile::tempdir().expect("isolated home");
    seed_home(home.path());
    let mut agent = McpProcess::start(home.path(), AGENT_A, Some(HOST_A));

    assert_eq!(agent.listed_panel_ids(), [SAME_WORKSPACE_PANEL]);
    let listed = agent.call("browser_list", &json!({}));
    assert_eq!(listed["structuredContent"]["panels"][0]["visible"], true);
    assert!(!listed.to_string().contains("browser_ws"));

    for panel_id in [OTHER_WORKSPACE_PANEL, OTHER_HOST_PANEL, STALE_STAMP_PANEL, LEGACY_PANEL] {
        assert_outside_workspace(&agent.call("browser_panel", &json!({ "panel_id": panel_id })), panel_id);
        assert_outside_workspace(&agent.call("browser_audit", &json!({ "panel_id": panel_id })), panel_id);
        assert_outside_workspace(
            &agent.call(
                "browser_navigate",
                &json!({ "panel_id": panel_id, "url": "https://example.test/moved", "timeout_millis": 1000 }),
            ),
            panel_id,
        );
        assert_outside_workspace(
            &agent.call(
                "browser_visibility",
                &json!({ "panel_id": panel_id, "visible": true, "timeout_millis": 1000 }),
            ),
            panel_id,
        );
        assert_outside_workspace(
            &agent.call(
                "browser_handoff",
                &json!({ "panel_id": panel_id, "reason": "cross-workspace attempt" }),
            ),
            panel_id,
        );
    }
    let other = read_manifest(home.path(), OTHER_WORKSPACE_PANEL);
    assert_eq!(
        other.owner,
        Some(stale_owner("horizon:agent-b")),
        "a rejected call must not claim ownership"
    );
    assert!(other.actions.is_empty());
    assert!(other.handoff.is_none());
    assert!(read_manifest(home.path(), LEGACY_PANEL).owner.is_none());

    let same = agent.call("browser_panel", &json!({ "panel_id": SAME_WORKSPACE_PANEL }));
    assert_eq!(same["isError"], false, "{same}");
    assert_eq!(same["structuredContent"]["panel_id"], SAME_WORKSPACE_PANEL);
    assert_eq!(same["structuredContent"]["owned_by_caller"], false);
    let handoff = agent.call(
        "browser_handoff",
        &json!({ "panel_id": SAME_WORKSPACE_PANEL, "reason": "please sign in" }),
    );
    assert_eq!(handoff["isError"], false, "{handoff}");
    assert_eq!(handoff["structuredContent"]["handoff_pending"], true);
    assert_eq!(
        read_manifest(home.path(), SAME_WORKSPACE_PANEL)
            .owner
            .map(|owner| owner.name),
        Some(AGENT_A.to_string()),
        "same-workspace reuse claims the panel"
    );

    agent.close();
}

#[test]
fn panel_moves_change_authorization_without_restarting_the_server() {
    let home = tempfile::tempdir().expect("isolated home");
    seed_home(home.path());
    let mut agent = McpProcess::start(home.path(), AGENT_A, Some(HOST_A));
    assert_eq!(agent.listed_panel_ids(), [SAME_WORKSPACE_PANEL]);

    // The host moved the other workspace's browser panel (or this agent) so
    // that both now share a workspace, and re-stamped the manifest.
    let mut moved_in = read_manifest(home.path(), OTHER_WORKSPACE_PANEL);
    moved_in.workspace = Some(ManifestWorkspace::new(
        HOST_A,
        "workspace-a",
        vec![AGENT_A.to_string(), "horizon:agent-b".to_string()],
    ));
    write_manifest(home.path(), &moved_in);
    // The host moved the original panel out of this agent's workspace.
    let mut moved_out = read_manifest(home.path(), SAME_WORKSPACE_PANEL);
    moved_out.workspace = Some(ManifestWorkspace::new(
        HOST_A,
        "workspace-c",
        vec!["horizon:agent-c".to_string()],
    ));
    write_manifest(home.path(), &moved_out);

    assert_eq!(agent.listed_panel_ids(), [OTHER_WORKSPACE_PANEL]);
    assert_outside_workspace(
        &agent.call("browser_panel", &json!({ "panel_id": SAME_WORKSPACE_PANEL })),
        SAME_WORKSPACE_PANEL,
    );
    let panel = agent.call("browser_panel", &json!({ "panel_id": OTHER_WORKSPACE_PANEL }));
    assert_eq!(panel["isError"], false, "{panel}");
    let handoff = agent.call(
        "browser_handoff",
        &json!({ "panel_id": OTHER_WORKSPACE_PANEL, "reason": "now shared" }),
    );
    assert_eq!(
        handoff["isError"], false,
        "a stale owner in the same workspace is reclaimable: {handoff}"
    );
    assert_eq!(
        read_manifest(home.path(), OTHER_WORKSPACE_PANEL)
            .owner
            .map(|owner| owner.name),
        Some(AGENT_A.to_string())
    );

    // A host that stops stamping the panel (for example an older build after
    // a downgrade) makes it disappear again rather than globally controllable.
    let mut unstamped = read_manifest(home.path(), OTHER_WORKSPACE_PANEL);
    unstamped.workspace = None;
    write_manifest(home.path(), &unstamped);
    assert!(agent.listed_panel_ids().is_empty());
    assert_outside_workspace(
        &agent.call("browser_audit", &json!({ "panel_id": OTHER_WORKSPACE_PANEL })),
        OTHER_WORKSPACE_PANEL,
    );

    agent.close();
}

#[test]
fn identities_from_outside_horizon_are_not_workspace_scoped() {
    let home = tempfile::tempdir().expect("isolated home");
    seed_home(home.path());
    let mut external = McpProcess::start(home.path(), "browser-cli-test", None);

    let mut listed = external.listed_panel_ids();
    listed.sort();
    assert_eq!(
        listed,
        [
            LEGACY_PANEL,
            OTHER_HOST_PANEL,
            OTHER_WORKSPACE_PANEL,
            SAME_WORKSPACE_PANEL,
            STALE_STAMP_PANEL
        ]
    );
    let legacy = external.call("browser_panel", &json!({ "panel_id": LEGACY_PANEL }));
    assert_eq!(legacy["isError"], false, "{legacy}");
    let create = external.call("browser_create", &json!({}));
    assert_eq!(create["isError"], true);
    assert!(
        create["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("only to an agent panel launched inside Horizon"))
    );

    external.close();
}

#[test]
fn a_horizon_identity_without_a_host_instance_fails_closed_with_a_clear_error() {
    let home = tempfile::tempdir().expect("isolated home");
    seed_home(home.path());
    let mut unbound = McpProcess::start(home.path(), AGENT_A, None);

    for (tool, arguments) in [
        ("browser_list", json!({})),
        ("browser_panel", json!({ "panel_id": SAME_WORKSPACE_PANEL })),
        ("browser_create", json!({})),
        (
            "browser_handoff",
            json!({ "panel_id": SAME_WORKSPACE_PANEL, "reason": "no host" }),
        ),
    ] {
        let result = unbound.call(tool, &arguments);
        assert_eq!(result["isError"], true, "{tool} must fail closed: {result}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains("host instance is unknown") && text.contains("HORIZON_BROWSER_HOST_INSTANCE"),
            "{tool} must explain the missing host instance: {text}"
        );
    }
    assert!(read_manifest(home.path(), SAME_WORKSPACE_PANEL).owner.is_none());

    unbound.close();
}
