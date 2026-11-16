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
use horizon_core::{Config, HorizonHome, PanelId, RuntimeState, SessionStore, StartupDecision, WorkspaceState};

use crate::app::HorizonApp;
use crate::app::test_support::{
    HELD_PANEL_ID, HELD_PROGRAM_MARKER, held_panel_state, held_program_launch, program_runs,
};
use crate::input;

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
    horizon_core::ui_thread::use_strict_mode();
    let config_path = folder.join("config.json");
    let home = HorizonHome::from_root(folder.join(".horizon"));
    let mut config = Config::default();
    config.features.keep_agents_through_restart = true;
    let ctx = Context::default();
    let mut app = HorizonApp::new_with_egui_context(
        &ctx,
        &config,
        config_path.clone(),
        SessionStore::new(home, config_path),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_state.clone()),
        },
        input::ObservedKeyboardInputs::default(),
        None,
    );
    app.node_cards.base = folder.join("cards");
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
