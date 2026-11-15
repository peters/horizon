//! The rows of the agents of a machine in the sidebar: their words through a turn and
//! an approval of the Codex stand-in, the summary of a folded machine, the line of a
//! lost link and the rows of a link that came back.

use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::{AgentState, MachineWatch, WatchLink};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{raw_input, run_app_frame, test_app};
use crate::test_egui::DiscardTextures;

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

/// The link of `machine` in the snapshot of `app`.
fn link(app: &mut HorizonApp, machine: &str) -> Option<WatchLink> {
    app.machine_watch
        .watch
        .snapshot()
        .machine(machine)
        .map(|state| state.link)
}

/// The texts that one frame of the sidebar draws, after a frame of the window took
/// the snapshot of the watch that its rows come from.
fn sidebar_texts(ctx: &Context, app: &mut HorizonApp) -> Vec<String> {
    run_app_frame(ctx, app);
    let input = raw_input([1200.0, 900.0], None);
    let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

/// One app with a stand-in `ssh`, a machine of the list and one Codex stand-in in a
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
fn the_agents_of_a_machine_are_rows_under_its_row_through_a_turn_and_an_approval() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the working turn of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    let texts = sidebar_texts(&ctx, &mut app);
    for word in ["build-1", "CX", "shop", "Working"] {
        assert!(
            texts.iter().any(|text| text == word),
            "the row of the agent says {word}: {texts:?}"
        );
    }

    server.send("hz-shop-1", "ask");
    wait_for(&ctx, &mut app, "the approval of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::NeedsYou)
    });
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text == "Needs you"),
        "the row of the agent says that it waits for the person: {texts:?}"
    );
}

#[test]
fn a_frame_whose_snapshot_did_not_change_builds_no_row_again() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the working turn of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    let _ = sidebar_texts(&ctx, &mut app);
    let read_from = app.machine_rows.read_from();
    let version = app.machine_watch.watch.snapshot().version;
    for _ in 0..3 {
        run_app_frame(&ctx, &mut app);
        let _ = sidebar_texts(&ctx, &mut app);
    }
    assert_eq!(
        app.machine_watch.watch.snapshot().version,
        version,
        "the spinner of the turn changes no state and no snapshot"
    );
    assert_eq!(
        app.machine_rows.read_from(),
        read_from,
        "a frame whose snapshot did not change builds no row of a machine again"
    );
}

#[test]
fn a_folded_machine_says_its_summary_in_place_of_its_rows() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the working turn of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    assert!(
        sidebar_texts(&ctx, &mut app).iter().any(|text| text == "shop"),
        "the agent shows under its machine"
    );

    app.machine_watch.toggle_fold("build-1");
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text == "1 working"),
        "a folded machine says how many of its agents work: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == "shop"),
        "a folded machine shows no agent row: {texts:?}"
    );

    app.machine_watch.toggle_fold("build-1");
    assert!(
        sidebar_texts(&ctx, &mut app).iter().any(|text| text == "shop"),
        "the row of the agent comes back"
    );
}

#[test]
fn a_lost_link_says_when_it_was_lost_and_how_many_agents_it_saw() {
    let Some(server) = Server::new() else {
        return;
    };
    let Some(ssh) = Ssh::unsteady(&server, 6) else {
        return;
    };
    let folder = tempfile::tempdir().expect("a folder of the test");
    let Some(command) = codex_command(folder.path()) else {
        return;
    };
    let shop = folder.path().join("shop");
    std::fs::create_dir_all(&shop).expect("the folder of the pane");
    server.session_in("hz-shop-1", &shop, &command);
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    wait_for(&ctx, &mut app, "the lost link", |app| {
        matches!(link(app, "build-1"), Some(WatchLink::Lost { .. }))
    });
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text.starts_with("link lost ")),
        "the row of the machine says when the link was lost: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text == "Last seen: 1 agent. Horizon tries again."),
        "the place of the rows says what the last report held: {texts:?}"
    );

    wait_for(&ctx, &mut app, "the link that came back", |app| {
        link(app, "build-1") == Some(WatchLink::Watching)
    });
    assert!(
        sidebar_texts(&ctx, &mut app).iter().any(|text| text == "shop"),
        "the row of the agent comes back with the link"
    );
}

#[test]
fn the_setting_off_removes_the_rows_of_every_machine() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the agent of the machine", |app| {
        agent_state(app, "hz-shop-1").is_some()
    });
    app.template_config.machines.watch_agents = false;
    wait_for(&ctx, &mut app, "the watch of the setting that is off", |app| {
        link(app, "build-1") == Some(WatchLink::Off)
    });
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text == "build-1"),
        "the row of the machine stays: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == "shop"),
        "the setting off removes the rows of the agents: {texts:?}"
    );
}
