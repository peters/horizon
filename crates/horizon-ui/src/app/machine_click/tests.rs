//! The terminal that a click on a machine row opens, judged on a test board.

use std::fs;

use egui::Context;
use horizon_core::{
    CanvasViewState, Machine, MachineKeys, MachineRegistry, Machines, PanelKind, RuntimeState, StartupDecision,
    WindowConfig,
};

use super::OPENED_FROM_MACHINES;
use super::*;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    // The startup read of the machine list answers before a test writes one.
    settle_machine_load(&mut app);
    (temp, ctx, app)
}

fn plant_key(app: &HorizonApp, name: &str) {
    let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    fs::create_dir_all(&directory).expect("the key directory");
    fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(directory.join(format!("{name}.pub")), "ssh-ed25519 AAAA\n").expect("the public key");
}

fn record(name: &str, session: Option<&str>) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        port: None,
        session: session.map(str::to_owned),
        provider: Some("Hetzner, hel1".to_owned()),
        added_at_millis: 1,
    }
}

/// Keeps `machines`, with the key that this computer uses for each of `keys`.
fn keep(app: &mut HorizonApp, machines: Vec<Machine>, keys: &[(&str, &str)]) {
    let mut list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), machines);
    for (machine, key) in keys {
        list = list.with_key(machine, key);
    }
    app.machine_list = list;
}

#[test]
fn a_click_opens_one_terminal_and_the_second_click_focuses_it() {
    let (_temp, ctx, mut app) = app();
    plant_key(&app, "build-1");
    keep(
        &mut app,
        vec![record("build-1", Some("work"))],
        &[("build-1", "build-1")],
    );
    let before = app.board.panels.len();

    app.connect_machine("build-1", &ctx);
    app.settle_browser_host_io();
    assert_eq!(app.board.panels.len(), before + 1, "the click opens one terminal");
    let panel = app
        .board
        .panels
        .iter()
        .find(|panel| panel.machine() == Some("build-1"))
        .expect("the terminal of the machine");
    assert_eq!(panel.opened_by(), Some(OPENED_FROM_MACHINES));
    assert_eq!(panel.kind, PanelKind::Ssh);
    let id = panel.id;
    assert_eq!(app.board.focused, Some(id), "the click focuses the terminal it opened");
    assert_eq!(
        panel
            .ssh_connection
            .as_ref()
            .and_then(|connection| connection.remote_command.as_deref()),
        Some("tmux new -A -s work"),
        "the click attaches the session of the record"
    );

    app.refresh_machine_rows();
    assert_eq!(
        app.machine_rows.rows()[0].panel,
        Some(id),
        "the row reports the terminal of the machine"
    );
    // The sidebar focuses the open terminal instead of opening a second one.
    app.board.focus(app.board.panels[0].id);
    let row = app.machine_rows.rows()[0].clone();
    assert_eq!(row.panel, Some(id));
}

#[test]
fn a_click_on_a_machine_without_a_key_opens_the_plain_login() {
    let (_temp, ctx, mut app) = app();
    keep(&mut app, vec![record("build-1", None)], &[]);

    app.connect_machine("build-1", &ctx);
    app.settle_browser_host_io();
    let panel = app
        .board
        .panels
        .iter()
        .find(|panel| panel.machine() == Some("build-1"))
        .expect("the terminal of the machine");
    let args = panel.ssh_connection.as_ref().expect("the connection").to_command_args();
    assert!(
        !args.iter().any(|argument| argument == "-i"),
        "a machine without a key offers the person's own keys: {args:?}"
    );
    assert_eq!(args.last().map(String::as_str), Some("root@203.0.113.10"));
}

#[test]
fn a_record_that_fails_revalidation_opens_nothing_and_says_why() {
    let (_temp, ctx, mut app) = app();
    let mut broken = record("build-1", Some("work"));
    broken.host = "-oProxyCommand=touch /var/tmp/hz-c9".to_owned();
    keep(&mut app, vec![broken], &[]);
    let before = app.board.panels.len();

    app.connect_machine("build-1", &ctx);
    app.settle_browser_host_io();
    assert_eq!(
        app.board.panels.len(),
        before,
        "a record that no longer validates opens nothing"
    );
    let notice = app.sidebar_notice.clone().expect("the sidebar says why");
    assert!(notice.contains("build-1"), "{notice}");
    assert!(notice.contains("invalid_ssh_route"), "{notice}");
}

#[test]
fn a_click_on_a_machine_that_is_gone_opens_nothing() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    app.connect_machine("build-1", &ctx);
    app.settle_browser_host_io();
    assert_eq!(app.board.panels.len(), before);
    let notice = app.sidebar_notice.clone().expect("the sidebar says why");
    assert!(notice.contains("machine_unavailable"), "{notice}");
}

#[test]
fn the_saved_panel_keeps_its_machine_and_its_session() {
    let (_temp, ctx, mut app) = app();
    plant_key(&app, "build-1");
    keep(
        &mut app,
        vec![record("build-1", Some("work"))],
        &[("build-1", "build-1")],
    );
    app.connect_machine("build-1", &ctx);
    app.settle_browser_host_io();

    let saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    let saved_panel = saved
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panels.iter())
        .find(|panel| panel.kind == PanelKind::Ssh)
        .expect("the saved terminal");
    assert_eq!(
        saved_panel.machine.as_deref(),
        Some("build-1"),
        "the tie to the machine is saved with the panel"
    );
    assert_eq!(
        saved_panel
            .to_panel_options(&horizon_core::browser::BrowserConfig::default())
            .machine
            .as_deref(),
        Some("build-1"),
        "a restored panel reads the tie back"
    );
    let encoded = serde_json::to_string(&saved).expect("serialize the state");
    let restored: RuntimeState = serde_json::from_str(&encoded).expect("parse the state");
    let restored_panel = restored
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panels.iter())
        .find(|panel| panel.kind == PanelKind::Ssh)
        .expect("the restored terminal");
    assert_eq!(restored_panel.machine.as_deref(), Some("build-1"));
    assert_eq!(
        restored_panel
            .ssh_connection
            .as_ref()
            .and_then(|connection| connection.remote_command.as_deref()),
        Some("tmux new -A -s work"),
        "the panel re-attaches to the same tmux session after a restart"
    );
}
