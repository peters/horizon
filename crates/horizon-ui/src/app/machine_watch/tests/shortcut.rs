//! The shortcut that goes to the next agent that waits for the person, which also
//! reaches the agents of a machine.

use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::{AgentState, MachineWatch};

use crate::app::HorizonApp;
use crate::app::test_support::test_app;
use crate::app::block_machines::settle_machine_load;

use super::support::{Server, Ssh, codex_command, record, wait_for, with_machines};

/// The state of the agent of `session` in the snapshot of `app`.
fn agent_state(app: &mut HorizonApp, session: &str) -> Option<AgentState> {
    let snapshot = app.machine_watch.watch.snapshot();
    snapshot
        .machines
        .iter()
        .flat_map(|machine| machine.agents.iter())
        .find(|agent| agent.session == session)
        .map(|agent| agent.state)
}

/// One app with a stand-in `ssh`, the machine `build-1` and one Codex stand-in in a
/// `tmux` pane of the folder `shop`.
fn app_with_one_agent() -> Option<(tempfile::TempDir, Server, Ssh, Context, HorizonApp)> {
    let server = Server::new()?;
    let ssh = Ssh::new(&server)?;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let command = codex_command(folder.path())?;
    let shop = folder.path().join("shop");
    std::fs::create_dir_all(&shop).expect("the folder of the pane");
    server.session_in("hz-shop-1", &shop, &command);
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    Some((folder, server, ssh, ctx, app))
}

#[test]
fn the_shortcut_reaches_the_agent_of_a_machine_that_waits_for_the_person() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "ask");
    wait_for(&ctx, &mut app, "the approval of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::NeedsYou)
    });
    assert!(app.board.panels.is_empty(), "no window shows an agent yet");

    app.go_to_next_attention(&ctx);
    wait_for(&ctx, &mut app, "the terminal of the waiting agent", |app| {
        !app.board.panels.is_empty()
    });
    let panel = app.board.panels.first().expect("the terminal of the agent");
    let panel_id = panel.id;
    assert_eq!(panel.machine(), Some("build-1"));
    assert_eq!(panel.machine_session(), Some("hz-shop-1"));
    assert_eq!(
        app.board.focused,
        Some(panel_id),
        "the shortcut gives the terminal the keyboard, as a press on the row does"
    );
    let workspace = app
        .board
        .workspace(panel.workspace_id)
        .expect("the workspace of the terminal");
    assert_eq!(
        workspace.name, "build-1",
        "the terminal stands in the workspace of the machine"
    );
}

#[test]
fn the_shortcut_says_that_nobody_waits_when_no_agent_waits() {
    let Some((_folder, _server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    assert_eq!(agent_state(&mut app, "hz-shop-1"), Some(AgentState::Idle));
    app.go_to_next_attention(&ctx);
    assert_eq!(
        app.sidebar_notice.as_deref(),
        Some(crate::app::attention::NOBODY_WAITS),
        "an idle machine agent is no reason to open a terminal"
    );
    assert!(app.board.panels.is_empty());
}
