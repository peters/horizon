//! A press on the row of an agent of a machine: the panel that it opens, the jump to a
//! panel that already shows the session, the agent that the person saw, and the row of
//! a session name that no terminal of this build opens.

use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::{AgentState, MachineWatch};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{raw_input, run_app_frame, test_app};
use crate::test_egui::DiscardTextures;
use horizon_core::StartupDecision;

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

/// One frame of the sidebar with `events`.
fn sidebar_frame(ctx: &Context, app: &mut HorizonApp, events: Vec<egui::Event>) {
    let mut input = raw_input([1200.0, 900.0], None);
    input.events = events;
    let _ = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
}

/// Presses the widget of the sidebar whose accessibility label starts with `label`.
fn press(ctx: &Context, app: &mut HorizonApp, label: &str) {
    ctx.enable_accesskit();
    // The rows of the agents come from the snapshot that a frame of the window reads.
    run_app_frame(ctx, app);
    let mut center = None;
    for _ in 0..2 {
        let input = raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        let nodes = output
            .platform_output
            .accesskit_update
            .expect("the accessibility tree of the sidebar");
        center = nodes.nodes.iter().find_map(|(_, node)| {
            let text = node.label().or_else(|| node.value());
            (text.is_some_and(|text| text.starts_with(label)))
                .then(|| node.bounds())
                .flatten()
        });
    }
    let bounds = center.unwrap_or_else(|| panic!("the control {label:?} has a position"));
    #[allow(clippy::cast_possible_truncation)]
    let position = egui::Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    );
    let button = |pressed: bool| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    sidebar_frame(ctx, app, vec![egui::Event::PointerMoved(position)]);
    sidebar_frame(ctx, app, vec![button(true)]);
    sidebar_frame(ctx, app, vec![button(false)]);
}

/// One app with a stand-in `ssh`, the machine `build-1` and one Codex stand-in in a
/// `tmux` pane of the folder `shop` of the session `session`.
///
/// `workspaces` seeds that many workspace rows, so the workspace of the machine is made
/// beside them on the canvas.
fn app_with_one_agent(session: &str) -> Option<(tempfile::TempDir, Server, Ssh, Context, HorizonApp)> {
    let server = Server::new()?;
    let ssh = Ssh::new(&server)?;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let command = codex_command(folder.path())?;
    let shop = folder.path().join("shop");
    std::fs::create_dir_all(&shop).expect("the folder of the pane");
    server.session_in(session, &shop, &command);
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    Some((folder, server, ssh, ctx, app))
}

#[test]
fn a_press_on_the_row_of_an_agent_opens_its_session_in_the_workspace_of_the_machine() {
    let Some((_folder, _server, _ssh, ctx, mut app)) = app_with_one_agent("hz-shop-1") else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    press(&ctx, &mut app, "Codex: shop");
    wait_for(&ctx, &mut app, "the terminal of the agent", |app| {
        !app.board.panels.is_empty()
    });
    let panel = app.board.panels.first().expect("the terminal of the agent");
    let panel_id = panel.id;
    assert_eq!(panel.kind, horizon_core::PanelKind::Ssh, "the panel is an SSH terminal");
    assert_eq!(panel.machine(), Some("build-1"));
    assert_eq!(panel.machine_session(), Some("hz-shop-1"));
    let workspace = app
        .board
        .workspace(panel.workspace_id)
        .expect("the workspace of the panel");
    assert_eq!(
        workspace.name, "build-1",
        "the terminal opens in the workspace of the machine"
    );
    let command = horizon_core::machines::session_command("hz-shop-1", None);
    let args = panel
        .ssh_connection
        .as_ref()
        .expect("the connection of the terminal")
        .to_command_args();
    assert_eq!(
        args,
        vec![
            "-p".to_owned(),
            "2222".to_owned(),
            "-o".to_owned(),
            "ConnectTimeout=10".to_owned(),
            "-o".to_owned(),
            "ServerAliveInterval=5".to_owned(),
            "-o".to_owned(),
            "ServerAliveCountMax=3".to_owned(),
            "-t".to_owned(),
            "127.0.0.1".to_owned(),
            command,
        ],
        "the command line of the terminal is the one of a machine terminal of this session"
    );

    // A second press goes to the terminal that stands, and opens no second one.
    let count = app.board.panels.len();
    press(&ctx, &mut app, "Codex: shop");
    assert_eq!(app.board.panels.len(), count, "a second press opens no second panel");
    assert_eq!(
        app.board.focused,
        Some(panel_id),
        "the second press goes to the terminal of the session"
    );
}

#[test]
fn a_press_on_the_row_of_an_agent_brings_its_new_terminal_into_view() {
    let Some((_folder, _server, _ssh, ctx, mut app)) = app_with_one_agent_beside_three_workspaces("hz-shop-1") else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    press(&ctx, &mut app, "Codex: shop");
    let session = Some("hz-shop-1");
    wait_for(&ctx, &mut app, "the terminal of the agent", |app| {
        app.board.panels.iter().any(|panel| panel.machine_session() == session)
    });
    let panel_id = app
        .board
        .panels
        .iter()
        .find(|panel| panel.machine_session() == session)
        .expect("the terminal of the agent")
        .id;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.panel_screen_rects.contains_key(&panel_id) {
        assert!(
            std::time::Instant::now() < deadline,
            "the view of the first press holds the terminal: the canvas is {:?} and the terminal is at {:?}",
            app.canvas_rect(&ctx),
            app.board
                .panel(panel_id)
                .map(|panel| (panel.layout.position, panel.layout.size))
        );
        run_app_frame(&ctx, &mut app);
    }
    let painted = app.panel_screen_rects[&panel_id];
    let canvas = app.canvas_rect(&ctx);
    assert!(
        canvas.contains_rect(painted),
        "the first press brings the terminal into view: the canvas holds {canvas:?} and the terminal is {painted:?}"
    );
    assert_eq!(
        app.board.focused,
        Some(panel_id),
        "the first press gives the terminal the keyboard, as the second one does"
    );
}

/// The app of one agent, beside three workspace rows of the board, so the workspace of the
/// machine is made beyond the view.
fn app_with_one_agent_beside_three_workspaces(
    session: &str,
) -> Option<(tempfile::TempDir, Server, Ssh, Context, HorizonApp)> {
    let server = Server::new()?;
    let ssh = Ssh::new(&server)?;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let command = codex_command(folder.path())?;
    let shop = folder.path().join("shop");
    std::fs::create_dir_all(&shop).expect("the folder of the pane");
    server.session_in(session, &shop, &command);
    let seeded = horizon_core::RuntimeState {
        workspaces: ["first", "second", "third"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                #[allow(clippy::cast_precision_loss)]
                let column = index as f32;
                crate::app::test_support::editor_workspace_state(name, [column * 900.0, 0.0])
            })
            .collect(),
        ..horizon_core::RuntimeState::default()
    };
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(seeded),
    });
    app.initial_pan_done = true;
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    run_app_frame(&ctx, &mut app);
    app.canvas_view = horizon_core::CanvasViewState::default();
    Some((folder, server, ssh, ctx, app))
}

#[test]
fn a_press_on_the_row_of_an_agent_marks_it_as_seen() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent("hz-shop-1") else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the end of the turn", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    server.send("hz-shop-1", "stop");
    wait_for(&ctx, &mut app, "the finished turn", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Done)
    });
    press(&ctx, &mut app, "Codex: shop");
    wait_for(&ctx, &mut app, "the terminal of the agent", |app| {
        !app.board.panels.is_empty()
    });
    wait_for(&ctx, &mut app, "the row of the agent that the person saw", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Idle)
    });
}

#[test]
fn the_row_of_a_session_name_that_no_terminal_opens_opens_nothing() {
    let Some((_folder, _server, _ssh, ctx, mut app)) = app_with_one_agent("hz flat") else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz flat").is_some()
    });
    press(&ctx, &mut app, "Codex: shop");
    for _ in 0..3 {
        run_app_frame(&ctx, &mut app);
    }
    assert!(
        app.board.panels.is_empty(),
        "a session name outside the rule of this build opens no terminal"
    );
}

#[test]
fn a_press_on_the_fold_control_of_a_machine_hides_its_agents() {
    let Some((_folder, _server, _ssh, ctx, mut app)) = app_with_one_agent("hz-shop-1") else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    assert!(!app.machine_watch.folded("build-1"), "a machine starts open");
    press(&ctx, &mut app, "Fold the agents of build-1");
    assert!(
        app.machine_watch.folded("build-1"),
        "the press on the fold control folds the agents of the machine"
    );
    press(&ctx, &mut app, "Show the agents of build-1");
    assert!(!app.machine_watch.folded("build-1"), "the press shows them again");
}
