//! The end of Horizon and the program of a holder: the program keeps running,
//! and the next start attaches to the same program.
//!
//! The holder serves on a thread of this process, so a test starts no second
//! Horizon. Every lane uses the id of a real panel and the path that the app
//! takes: a saved board, the coordination worker, and the exit of a window.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::panel_holder;
use horizon_core::{HorizonHome, PanelId, RuntimeState, StartupDecision, WorkspaceState};

use crate::app::HorizonApp;
use crate::app::test_support::{
    HELD_PANEL_ID, HELD_PROGRAM_MARKER, app_in, editor_panel_state, held_panel_state, held_program_launch,
    keep_agents_config, program_runs, run_app_frame,
};

/// The id of the one panel of a board that does not show a held panel.
const OTHER_PANEL_ID: &str = "0e4f2a41-6d68-4c1c-9d95-5c8d3f2f7c11";

/// The line of a start that ended the program of one holder that no panel showed.
const SENTENCE: &str = "Horizon ended 1 agent program that no panel showed.";

/// The longest time that a test waits for one condition.
const TEST_BOUND: Duration = Duration::from_secs(20);

/// The saved board of one agent panel that continues its work after a restart.
fn saved_board() -> RuntimeState {
    RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_owned(),
            name: "workspace".to_owned(),
            panels: vec![held_panel_state(HELD_PANEL_ID)],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}

/// An app on the home of `folder` with the setting that keeps an agent through a
/// restart, with every panel of its saved board started.
fn app_on(folder: &Path, runtime_state: &RuntimeState) -> (Context, HorizonApp) {
    let ctx = Context::default();
    let mut app = app_in(
        &ctx,
        folder,
        &keep_agents_config(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_state.clone()),
        },
    );
    app.settle_panel_starts();
    (ctx, app)
}

/// The panel of the saved board, whose program a holder runs.
fn held_panel(app: &HorizonApp) -> PanelId {
    app.board
        .panels
        .iter()
        .find(|panel| panel.local_id == HELD_PANEL_ID)
        .map(|panel| panel.id)
        .expect("the panel of the saved board")
}

/// The pid of the program that the terminal of `panel` shows.
fn pid_of(app: &HorizonApp, panel: PanelId) -> u32 {
    app.board
        .panel(panel)
        .and_then(horizon_core::Panel::terminal)
        .and_then(horizon_core::Terminal::child_pid)
        .expect("the pid of the program of a panel")
}

/// Reads the grid until it holds `wanted`, and answers its text.
fn wait_for_text(app: &mut HorizonApp, panel: PanelId, wanted: &str) -> String {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        let _ = app.drain_panel_output();
        let text = app
            .board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .map_or_else(String::new, |terminal| terminal.last_lines_text(40));
        if text.contains(wanted) {
            return text;
        }
        assert!(Instant::now() < deadline, "waited for {wanted:?}, read {text:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The board of one editor panel, which does not show the panel of a holder.
fn board_without_the_held_panel() -> RuntimeState {
    RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_owned(),
            name: "workspace".to_owned(),
            panels: vec![editor_panel_state(OTHER_PANEL_ID, [40.0, 40.0])],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}

/// A start on a board that does not show the panel of a running holder ends its
/// program, and the sidebar says so: the person reads what the start took away.
#[test]
fn a_start_that_ends_the_program_of_an_unshown_holder_says_so_in_the_sidebar() {
    let folder = crate::app::test_support::short_temp("hz-held-unshown");
    let home = HorizonHome::from_root(folder.path().join(".horizon"));
    let hello =
        panel_holder::serve_on_a_thread(&home, &held_program_launch(HELD_PANEL_ID)).expect("the holder of a test");
    assert!(
        program_runs(hello.pid),
        "the program of the holder runs before the start"
    );
    // The start of the application: it reads the board of the last run from the
    // store, as `prepare_startup` does for the window.
    let config = horizon_core::Config::default();
    let store = horizon_core::SessionStore::new(home.clone(), folder.path().join("config.json"));
    store
        .create_session_from_runtime(board_without_the_held_panel())
        .expect("the session of the last run");
    let startup = store.prepare_startup(&config).expect("the decision of a start");
    let ctx = Context::default();
    let mut app = app_in(&ctx, folder.path(), &config, startup);
    app.settle_panel_starts();

    let says = |app: &HorizonApp| {
        [&app.sidebar_notice, &app.sidebar_sticky_notice]
            .into_iter()
            .flatten()
            .any(|line| line.contains(SENTENCE))
    };
    let deadline = Instant::now() + TEST_BOUND;
    while !says(&app) {
        assert!(
            Instant::now() < deadline,
            "the sidebar says nothing about the holder that the start ended: {:?} {:?}",
            app.sidebar_notice,
            app.sidebar_sticky_notice
        );
        run_app_frame(&ctx, &mut app);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !program_runs(hello.pid),
        "the start ends the program of the unshown holder"
    );
    // The person looks at the window some time after the start, as the live run
    // did: 15 seconds later the line must still be there.
    app.notice_clock.ahead += Duration::from_secs(15);
    run_app_frame(&ctx, &mut app);
    assert!(
        says(&app),
        "the line is gone 15 seconds after the start, before a person read it"
    );
}

/// The end of Horizon detaches from the program of a holder and signals nothing:
/// the same program serves the panel of the next start, and the panel offers no
/// conversation to resume, because that conversation still runs in it.
#[test]
fn the_end_of_horizon_keeps_the_program_of_a_holder_and_the_next_start_attaches() {
    let folder = crate::app::test_support::short_temp("hz-held-exit");
    let home = HorizonHome::from_root(folder.path().join(".horizon"));
    let hello =
        panel_holder::serve_on_a_thread(&home, &held_program_launch(HELD_PANEL_ID)).expect("the holder of a test");
    let board = saved_board();

    let (_ctx, mut app) = app_on(folder.path(), &board);
    let panel = held_panel(&app);
    assert_eq!(
        pid_of(&app, panel),
        hello.pid,
        "the panel of the saved board attaches to the program of the holder"
    );
    wait_for_text(&mut app, panel, HELD_PROGRAM_MARKER);

    // The end of the application, as `on_exit` runs it.
    app.begin_shutdown();
    app.run_exit_cleanup();
    // Longer than the hangup and the two steps that the end of a terminal gives a
    // program that outlives it, so a signal of this app would have landed.
    std::thread::sleep(horizon_core::AFTER_HANGUP + horizon_core::AFTER_TERM + Duration::from_millis(500));
    assert!(
        program_runs(hello.pid),
        "the program of a holder outlives the end of Horizon (pid {})",
        hello.pid
    );
    // The exit releases the link of the panel on its own thread, so the holder
    // serves the next client once that link is gone. The link of the terminal is
    // its socket, which this app closes with the panel or with its process.
    drop(app);
    let deadline = Instant::now() + TEST_BOUND;
    while !matches!(panel_holder::probe(&home, HELD_PANEL_ID), Ok(Some(_))) {
        assert!(
            Instant::now() < deadline,
            "the holder of the panel still answers after the end of Horizon"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let (_ctx, mut after) = app_on(folder.path(), &board);
    let restored = held_panel(&after);
    assert_eq!(
        pid_of(&after, restored),
        hello.pid,
        "the next start attaches to the same program"
    );
    assert!(
        wait_for_text(&mut after, restored, HELD_PROGRAM_MARKER).contains(HELD_PROGRAM_MARKER),
        "the grid of the restored panel shows the output of the program"
    );
    assert!(
        after
            .board
            .panel(restored)
            .and_then(horizon_core::Panel::terminal)
            .and_then(horizon_core::Terminal::pending_work_resume)
            .is_none(),
        "a conversation that still runs in its panel is no work to resume"
    );
}
