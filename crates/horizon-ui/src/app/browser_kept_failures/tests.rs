use horizon_core::browser::manifest::{self, AgentIdentity};
use horizon_core::browser::{AttachedHost, BrowserPanelState};
use horizon_core::{Panel, PanelContent, PanelId, PanelKind, WorkspaceId};

use crate::app::{HorizonApp, test_support};

/// A browser panel of a browser host that lost its browser after it showed a
/// page, placed like one the canvas holds.
fn kept_panel(app: &mut HorizonApp, workspace: WorkspaceId, id: u64) -> PanelId {
    let id = PanelId(id);
    let state = BrowserPanelState::lost_attached_host_for_tests(
        &AttachedHost::new("browser-vm", Some("mail")).expect("one SSH name"),
        "https://example.test/inbox",
        "CDP connection lost: websocket: WebSocket protocol error: Connection reset without closing handshake",
    );
    let mut panel = Panel::from_content(
        id,
        workspace,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(state)),
    );
    panel.visible = true;
    app.board.panels.push(panel);
    app.board.workspace_mut(workspace).expect("workspace").add_panel(id);
    id
}

fn agent_identity(actor: &str) -> AgentIdentity<'_> {
    AgentIdentity::new(actor, Some(manifest::host_instance()))
}

#[test]
fn a_kept_panel_publishes_its_typed_reason_for_the_agents_of_its_workspace() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let panel = kept_panel(&mut app, workspace, 100);
    let path = manifest::manifest_path_for_root(
        app.host_manifest_root(),
        &app.board.panel(panel).expect("panel").local_id,
    );

    assert!(app.sync_kept_browser_failures());
    app.settle_browser_host_io();

    let manifest = manifest::read_at(&path).expect("the reason the agents read");
    let failure = manifest.failure.expect("a typed reason");
    assert_eq!(failure.code, "attached_host_connection_lost");
    assert!(
        failure.message.contains("Connection reset without closing handshake"),
        "the agent reads the words of the lost connection: {}",
        failure.message
    );
    assert_eq!(manifest.url, "https://example.test/inbox");
    assert!(
        !manifest.hidden && manifest.workspace.is_some(),
        "the panel keeps its place and its workspace"
    );
    assert!(
        app.board
            .panel(panel)
            .expect("panel")
            .browser()
            .expect("browser")
            .attached_host_failure()
            .is_some(),
        "the panel itself still says why"
    );
    assert!(!app.sync_kept_browser_failures(), "an unchanged reason is written once");
}

#[test]
fn a_kept_panel_that_leaves_the_board_removes_its_reason() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let panel = kept_panel(&mut app, workspace, 100);
    let local_id = app.board.panel(panel).expect("panel").local_id.clone();
    let path = manifest::manifest_path_for_root(app.host_manifest_root(), &local_id);
    app.sync_kept_browser_failures();
    app.settle_browser_host_io();
    assert!(manifest::read_at(&path).is_some());

    app.board.close_panel(panel);
    assert!(app.sync_kept_browser_failures(), "the reason is released");
    app.settle_browser_host_io();

    assert!(
        manifest::read_at(&path).is_none(),
        "a panel that left the board is not in the agent's list"
    );
}

#[test]
fn a_kept_panel_that_serves_a_page_again_drops_the_reason_and_keeps_its_place() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let panel = kept_panel(&mut app, workspace, 100);
    let local_id = app.board.panel(panel).expect("panel").local_id.clone();
    let path = manifest::manifest_path_for_root(app.host_manifest_root(), &local_id);
    app.sync_kept_browser_failures();
    app.settle_browser_host_io();
    assert!(manifest::read_at(&path).expect("manifest").failure.is_some());

    let browser = app
        .board
        .panel_mut(panel)
        .expect("panel")
        .browser_mut()
        .expect("browser");
    *browser = BrowserPanelState::inert();
    browser.status = horizon_core::browser::BrowserStatus::Ready;

    assert!(app.sync_kept_browser_failures(), "the reason is released");
    app.settle_browser_host_io();

    let manifest = manifest::read_at(&path).expect("the panel keeps its manifest");
    assert_eq!(manifest.failure, None, "a panel that serves a page needs no reason");
    assert!(
        !manifest.authorizes(agent_identity("horizon:someone")),
        "the manifest stays readable only through its workspace stamp"
    );
}
