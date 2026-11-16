//! The rows of the agents of a machine in the sidebar: their words through a turn and
//! an approval of the Codex stand-in, the summary of a folded machine, the line of a
//! lost link and the rows of a link that came back.

use std::sync::Arc;

use egui::Context;
use horizon_core::machine_watch::{AgentState, MachineWatch, WatchLink};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{raw_input, run_app_frame, run_app_frame_with_input, test_app};
use crate::test_egui::DiscardTextures;

use super::support::{Server, Ssh, codex_command, record, wait_for, with_machines};

/// The state of the agent of `session` in the snapshot that the last frame of `app`
/// read: a lane reads the value that the frame looked at, and not a snapshot that the
/// watch of the machine wrote while the lane read it.
fn agent_state(app: &mut HorizonApp, session: &str) -> Option<AgentState> {
    let snapshot = app.machine_watch.snapshot.clone()?;
    snapshot
        .machines
        .iter()
        .flat_map(|machine| machine.agents.iter())
        .find(|agent| agent.session == session)
        .map(|agent| agent.state)
}

/// The link of `machine` in the snapshot that the last frame of `app` read.
fn link(app: &mut HorizonApp, machine: &str) -> Option<WatchLink> {
    app.machine_watch
        .snapshot
        .clone()?
        .machine(machine)
        .map(|state| state.link)
}

/// The texts of one frame of the sidebar.
fn texts_of(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

/// The texts that one frame of the sidebar draws, after a frame of the window took
/// the snapshot of the watch that its rows come from.
fn sidebar_texts(ctx: &Context, app: &mut HorizonApp) -> Vec<String> {
    run_app_frame(ctx, app);
    let input = raw_input([1200.0, 900.0], None);
    let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
    texts_of(&output)
}

/// Where one frame of the window draws the text `wanted`, in screen points.
fn window_text_pos(ctx: &Context, app: &mut HorizonApp, wanted: &str) -> Option<egui::Pos2> {
    let output = run_app_frame_with_input(ctx, app, raw_input([1200.0, 900.0], None));
    output.shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::epaint::Shape::Text(text) if text.galley.job.text == wanted => Some(text.pos),
        _ => None,
    })
}

/// The texts that one frame of the window draws at `seconds`, with the pointer moved to
/// `pointer` when one is given.
fn window_texts_at(ctx: &Context, app: &mut HorizonApp, pointer: Option<egui::Pos2>, seconds: f64) -> Vec<String> {
    let mut input = raw_input([1200.0, 900.0], None);
    input.time = Some(seconds);
    input.events = pointer.map(egui::Event::PointerMoved).into_iter().collect();
    texts_of(&run_app_frame_with_input(ctx, app, input))
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
    assert!(
        texts.iter().any(|text| text == "\u{25BE}"),
        "a machine with a line under its row paints the mark of the fold: {texts:?}"
    );
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
fn a_turn_that_ends_in_the_panel_of_the_session_that_has_the_focus_is_no_done() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the working turn of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    // No panel shows the session, so the person did not look at its turn.
    server.send("hz-shop-1", "stop");
    wait_for(&ctx, &mut app, "the finished turn that the person did not see", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Done)
    });
    assert!(
        sidebar_texts(&ctx, &mut app).iter().any(|text| text == "Done"),
        "a turn that ends while no panel of its session has the focus is Done"
    );

    // The press of the row opens the terminal of the session and gives it the keyboard.
    let agent = app
        .machine_watch
        .watch
        .snapshot()
        .agents("build-1")
        .first()
        .cloned()
        .expect("the row of the agent");
    app.open_machine_agent(&agent, &ctx);
    wait_for(&ctx, &mut app, "the terminal of the agent", |app| {
        app.board
            .panels
            .iter()
            .any(|panel| panel.machine_session() == Some("hz-shop-1"))
    });
    wait_for(&ctx, &mut app, "the row of the agent that the person saw", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Idle)
    });

    // The turn of the panel that has the focus ends while the focus stays there.
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the second working turn", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    server.send("hz-shop-1", "stop");
    wait_for(&ctx, &mut app, "the end of the turn of the focused panel", |app| {
        agent_state(app, "hz-shop-1") != Some(AgentState::Working)
    });
    // One more frame looks at the newest snapshot of the watch, so the row of the turn
    // that ended is the row of a frame that looked after it ended.
    run_app_frame(&ctx, &mut app);
    assert_ne!(
        agent_state(&mut app, "hz-shop-1"),
        Some(AgentState::Done),
        "a turn that ends in the panel of the session that has the focus is no Done"
    );
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        !texts.iter().any(|text| text == "Done"),
        "the row of the agent shows no Done while its panel has the focus: {texts:?}"
    );
}

#[test]
fn a_pane_that_reports_a_state_says_needs_you_before_the_title_of_its_agent() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.send("hz-shop-1", "start");
    wait_for(&ctx, &mut app, "the working turn of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::Working)
    });
    // The title of the stand-in says that its turn runs, and the program of the pane
    // reports that the turn waits for the person.
    server.report("hz-shop-1", Some("blocked"));
    wait_for(&ctx, &mut app, "the state that the pane reports", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::NeedsYou)
    });
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text == "Needs you"),
        "the report of the pane answers before the working title: {texts:?}"
    );
    // A word outside the four of the contract is no state, and the title answers again.
    server.report("hz-shop-1", Some("blocked please"));
    wait_for(
        &ctx,
        &mut app,
        "the title rule after a word outside the contract",
        |app| agent_state(app, "hz-shop-1") == Some(AgentState::Working),
    );
    server.report("hz-shop-1", None);
    let texts = sidebar_texts(&ctx, &mut app);
    assert!(
        texts.iter().any(|text| text == "Working"),
        "a state that the program took back leaves the title rule: {texts:?}"
    );
}

#[test]
fn a_job_that_a_session_reports_is_a_row_of_its_machine() {
    let Some((_folder, server, _ssh, ctx, mut app)) = app_with_one_agent() else {
        return;
    };
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "hz-night-1",
        "-x",
        "100",
        "-y",
        "20",
        "sleep",
        "300",
    ]);
    // The pane of the job runs no agent program, and its report says that it works.
    server.report("hz-night-1", Some("working"));
    wait_for(&ctx, &mut app, "the job of the machine", |app| {
        agent_state(app, "hz-night-1") == Some(AgentState::Working)
    });
    let texts = sidebar_texts(&ctx, &mut app);
    for word in ["hz-night-1", ">_", "Working"] {
        assert!(
            texts.iter().any(|text| text == word),
            "the row of the job says {word}: {texts:?}"
        );
    }
    let job = app
        .machine_watch
        .watch
        .snapshot()
        .agents("build-1")
        .iter()
        .find(|agent| agent.session == "hz-night-1")
        .cloned()
        .expect("the row of the job");
    assert!(job.can_open(), "a press on the row of the job opens its session");
    server.report("hz-night-1", Some("blocked"));
    wait_for(&ctx, &mut app, "the job that waits for the person", |app| {
        agent_state(app, "hz-night-1") == Some(AgentState::NeedsYou)
    });
    assert!(
        sidebar_texts(&ctx, &mut app).iter().any(|text| text == "Needs you"),
        "the row of the job says that it waits for the person"
    );
    // The report leaves with the job, and the row leaves with the report.
    server.report("hz-night-1", None);
    wait_for(&ctx, &mut app, "the row of the job to leave", |app| {
        agent_state(app, "hz-night-1").is_none()
    });
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
        texts.iter().any(|text| text == "1 agent \u{00B7} trying again"),
        "the place of the rows says what the last report held in words that fit: {texts:?}"
    );
    // The whole sentence is the hover of that line.
    let at = window_text_pos(&ctx, &mut app, "1 agent \u{00B7} trying again").expect("the quiet line of the row");
    let pointer = at + egui::vec2(10.0, 5.0);
    let mut hovered = Vec::new();
    for (pointer, seconds) in [(Some(pointer), 0.0), (None, 1.0), (None, 1.5)] {
        hovered = window_texts_at(&ctx, &mut app, pointer, seconds);
    }
    assert!(
        hovered
            .iter()
            .any(|text| text == "Last seen: 1 agent. Horizon tries again."),
        "the whole sentence is in the hover of the line: {hovered:?}"
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
    assert!(
        !texts.iter().any(|text| text == "\u{25BE}" || text == "\u{25B8}"),
        "a machine with no line under its row paints no fold mark: {texts:?}"
    );
}
