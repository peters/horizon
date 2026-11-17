//! The decision of a holder, proven against a holder that serves on a thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::panel_holder::{self, HolderError, Launch};
use crate::terminal::Terminal;
use crate::{Board, HorizonHome, Panel, PanelId, PanelKind, PanelOptions, PanelResume, RuntimeState, WorkspaceId};

use super::super::start::{StartRequest, finish_restart, prepare_panel_start, prepare_restart};
use super::super::terminal::spawn_started_terminal;
use super::{CLEAR_BOUND, started_within};

/// The marker that the stand-in program of these tests prints.
const MARKER: &str = "held marker";

/// The longest time that a test waits for one condition of a panel.
const TEST_BOUND: Duration = Duration::from_secs(10);

/// A home in a folder of a short path, because the path of a socket is bounded.
fn temp_home() -> (tempfile::TempDir, HorizonHome) {
    let folder = tempfile::Builder::new()
        .prefix("hz-hold")
        .tempdir_in("/tmp")
        .expect("a folder for a home");
    let home = HorizonHome::from_root(folder.path().to_path_buf());
    (folder, home)
}

/// The options of the panel of a test: a stand-in program that prints a marker.
fn panel_options(kind: PanelKind, local_id: &str, is_restore: bool) -> PanelOptions {
    PanelOptions {
        kind,
        local_id: Some(local_id.to_owned()),
        is_restore,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), format!("printf '{MARKER}\\n'; exec cat")],
        cwd: Some(PathBuf::from("/tmp")),
        resume: PanelResume::Fresh,
        cols: 400,
        ..PanelOptions::default()
    }
}

/// The launch of the stand-in program of a test, for a holder that it serves.
fn stand_in(local_id: &str) -> Launch {
    Launch {
        local_id: local_id.to_owned(),
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), format!("printf '{MARKER}\\n'; exec cat")],
        cwd: Some(PathBuf::from("/tmp")),
        env: HashMap::new(),
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 17,
    }
}

/// Starts a panel as the app starts it, and answers it with the arguments of its
/// launch and whether it attached to a holder.
fn start_panel(home: &HorizonHome, opts: PanelOptions, local_id: &str, keep: bool) -> (Panel, Vec<String>, bool) {
    let id = PanelId(1);
    let start = prepare_panel_start(StartRequest {
        id,
        local_id: local_id.to_owned(),
        opts: opts.clone(),
        home: home.clone(),
        keep_agents_through_restart: keep,
    });
    let launch = start.launch.as_ref().expect("the launch of the panel of a test");
    let args = launch.args.clone();
    let attached = launch.attached.is_some();
    let panel = spawn_started_terminal(id, WorkspaceId(1), local_id.to_owned(), opts, start)
        .expect("the panel of a test starts");
    (panel, args, attached)
}

/// Waits until the grid of `panel` holds `wanted`, and answers its text.
fn wait_for_text(panel: &mut Panel, wanted: &str) -> String {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        let text = match panel.terminal_mut() {
            Some(terminal) => {
                terminal.process_events();
                terminal.try_last_lines_text(40).unwrap_or_default()
            }
            None => String::new(),
        };
        if text.contains(wanted) {
            return text;
        }
        assert!(Instant::now() < deadline, "waited for {wanted:?}, read {text:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until `condition` holds, and fails the test when it does not.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + TEST_BOUND;
    while !condition() {
        assert!(Instant::now() < deadline, "waited for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// The pid of the program that the terminal of `panel` shows.
fn pid_of(panel: &Panel) -> u32 {
    panel
        .terminal()
        .and_then(Terminal::child_pid)
        .expect("the pid of the program of a panel")
}

#[test]
fn a_panel_of_a_computer_that_keeps_no_agents_starts_no_holder() {
    let (_folder, home) = temp_home();
    let (mut panel, args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Codex, "off-panel", false),
        "off-panel",
        false,
    );

    assert!(!attached, "the program of this panel runs in this process");
    assert_eq!(
        args.first().map(String::as_str),
        Some("-ic"),
        "an agent panel starts through the login shell of this computer, as it does today"
    );
    assert!(
        args.get(1).is_some_and(|line| line.contains("exec cat")),
        "the program of the panel is in the command line of the shell: {args:?}"
    );
    assert!(
        !home.root().join("panel-holders").exists(),
        "a run that keeps no agent makes no folder of holders"
    );
    assert!(
        matches!(panel_holder::probe(&home, "off-panel"), Ok(None)),
        "no holder answers for the panel"
    );

    wait_for_text(&mut panel, MARKER);
}

#[test]
fn an_agent_panel_of_a_computer_that_keeps_its_agents_starts_in_a_holder() {
    let (_folder, home) = temp_home();
    let (mut panel, _args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Codex, "held-panel", false),
        "held-panel",
        true,
    );

    assert!(attached, "the panel attaches to the holder of its program");
    assert!(
        !matches!(panel_holder::probe(&home, "held-panel"), Ok(None)),
        "a holder answers the probe of the panel, which attached to it"
    );
    let pid = pid_of(&panel);
    wait_for_text(&mut panel, MARKER);

    let (mut shell, _args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Shell, "shell-panel", false),
        "shell-panel",
        true,
    );
    assert!(!attached, "a shell panel runs its program in this process");
    assert!(
        matches!(panel_holder::probe(&home, "shell-panel"), Ok(None)),
        "a shell panel makes no holder"
    );
    wait_for_text(&mut shell, MARKER);
    assert_ne!(pid_of(&shell), pid, "the shell is a program of this process");
}

/// The promise of the round: the end of Horizon leaves the program in its holder,
/// and the next Horizon attaches to the same program and reads its last output.
#[test]
fn a_restored_agent_panel_attaches_to_the_program_that_its_holder_keeps() {
    let (_folder, home) = temp_home();
    let (mut panel, _args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Codex, "promise", false),
        "promise",
        true,
    );
    assert!(attached, "the panel attaches to the holder of its program");
    let pid = pid_of(&panel);
    wait_for_text(&mut panel, MARKER);
    // The end of Horizon: the terminal goes, and the holder keeps its program.
    drop(panel);

    let mut restoring = panel_options(PanelKind::Codex, "promise", true);
    // A panel that a person restarts resumes its conversation, which a program
    // that runs already must not be given.
    restoring.resume = PanelResume::Session {
        session_id: "4d7a1a44-0000-4000-8000-000000000000".to_owned(),
    };
    let (mut restored, args, attached) = start_panel(&home, restoring, "promise", true);
    assert!(
        attached,
        "the restored panel attaches to the holder that runs its program"
    );
    assert_eq!(pid_of(&restored), pid, "the same program serves both Horizons");
    assert!(
        wait_for_text(&mut restored, MARKER).contains(MARKER),
        "the grid of the restored panel shows the marker of the program"
    );
    assert!(
        args.is_empty(),
        "no resume argument is built for a program that runs: {args:?}"
    );
}

/// A holder that serves another client: the panel runs its program here, and one
/// line says why.
#[test]
fn a_holder_that_serves_another_client_leaves_the_program_in_this_window() {
    let (_folder, home) = temp_home();
    panel_holder::serve_on_a_thread(&home, &stand_in("busy-panel")).expect("the holder of a test");
    let client = panel_holder::linked(&home, "busy-panel")
        .expect("the link of the holder")
        .expect("a holder answers");
    wait_until("the holder to serve the client of the test", || {
        matches!(panel_holder::probe(&home, "busy-panel"), Err(HolderError::Busy))
    });

    let (mut panel, _args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Codex, "busy-panel", true),
        "busy-panel",
        true,
    );
    assert!(!attached, "the panel runs its program in this process");
    let text = wait_for_text(&mut panel, "This program runs in this window");
    assert!(
        text.contains("serves another client"),
        "the line says why the holder does not serve this panel: {text:?}"
    );
    drop(client);
}

/// A holder whose socket path is above the bound of a system: the panel runs its
/// program here, and one line says why. The path is never cut.
#[test]
fn a_holder_of_a_path_that_is_too_long_leaves_the_program_in_this_window() {
    let folder = tempfile::Builder::new()
        .prefix("hz-hold")
        .tempdir_in("/tmp")
        .expect("a folder for a home");
    let long = Path::new("/tmp")
        .join("x".repeat(96))
        .join(folder.path().file_name().expect("the name of the folder"));
    let home = HorizonHome::from_root(long);

    let (mut panel, _args, attached) = start_panel(
        &home,
        panel_options(PanelKind::Codex, "long-path", true),
        "long-path",
        true,
    );
    assert!(!attached, "the panel runs its program in this process");
    let text = wait_for_text(&mut panel, "This program runs in this window");
    assert!(
        text.contains("is longer than 100 bytes"),
        "the line names the bound of the path: {text:?}"
    );
}

/// A board with one workspace and one agent panel of a stand-in program, started
/// as the app starts it.
fn board_with_a_held_panel(home: &HorizonHome, local_id: &str, keep: bool) -> (Board, PanelId) {
    let state = RuntimeState {
        workspaces: vec![crate::WorkspaceState {
            local_id: "workspace".to_owned(),
            name: "workspace".to_owned(),
            panels: vec![crate::PanelState {
                local_id: local_id.to_owned(),
                name: local_id.to_owned(),
                kind: PanelKind::Codex,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), format!("printf '{MARKER}\\n'; exec cat")],
                rows: 24,
                cols: 400,
                ..crate::PanelState::default()
            }],
            ..crate::WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let mut board = Board::from_runtime_state_waiting(&state, None).expect("the restored board");
    for pending in board.take_pending_starts() {
        let request = StartRequest {
            id: pending.id,
            local_id: pending.options.local_id.clone().unwrap_or_default(),
            opts: pending.options.clone(),
            home: home.clone(),
            keep_agents_through_restart: keep,
        };
        let start = prepare_panel_start(request);
        board
            .start_panel(pending.id, &pending.options, start)
            .expect("the panel starts");
    }
    let id = board.panel_id_by_local_id(local_id).expect("the panel of the board");
    (board, id)
}

/// Whether a program of `pid` still runs.
fn runs(pid: u32) -> bool {
    crate::process::is_alive(pid)
}

#[test]
fn closing_the_panel_ends_the_program_of_its_holder() {
    let (_folder, home) = temp_home();
    let (mut board, id) = board_with_a_held_panel(&home, "closed-panel", true);
    let pid = board
        .panel(id)
        .and_then(Panel::terminal)
        .and_then(Terminal::child_pid)
        .expect("the pid of the program of the panel");
    assert!(
        matches!(panel_holder::probe(&home, "closed-panel"), Err(HolderError::Busy)),
        "a holder serves this panel before it closes"
    );

    board.close_panel(id);
    wait_until("the holder of the closed panel to go", || {
        matches!(panel_holder::probe(&home, "closed-panel"), Ok(None))
    });
    assert!(
        !home
            .root()
            .join("panel-holders")
            .join(format!("{}.sock", panel_holder::socket_stem("closed-panel")))
            .exists(),
        "the holder removed the socket of the closed panel"
    );
    assert!(!runs(pid), "the program of the closed panel ended");
}

#[test]
fn closing_the_workspace_ends_the_program_of_its_holder() {
    let (_folder, home) = temp_home();
    let (mut board, id) = board_with_a_held_panel(&home, "closed-workspace", true);
    let workspace = board.panel(id).expect("the panel").workspace_id;
    let pid = board
        .panel(id)
        .and_then(Panel::terminal)
        .and_then(Terminal::child_pid)
        .expect("the pid of the program of the panel");

    board.close_panels_in_workspace(workspace);
    wait_until("the holder of the closed workspace to go", || {
        matches!(panel_holder::probe(&home, "closed-workspace"), Ok(None))
    });
    assert!(!runs(pid), "the program of the closed workspace ended");
}

#[test]
fn a_restart_of_the_panel_starts_a_new_holder_with_another_program() {
    let (_folder, home) = temp_home();
    let (mut board, id) = board_with_a_held_panel(&home, "restarted-panel", true);
    let before = board
        .panel(id)
        .and_then(Panel::terminal)
        .and_then(Terminal::child_pid)
        .expect("the pid of the program of the panel");

    let request = board
        .panel_mut(id)
        .expect("the panel")
        .restart_request(home.clone(), true)
        .expect("the pre-flight of the restart")
        .expect("a panel that restarts");
    let preflight = prepare_restart(request).expect("the pre-flight of a worker");
    let wanted = preflight.work_brief_wanted();
    let pending = preflight.into_pending().expect("a panel whose process ends first");
    board.begin_restart(id, wanted).expect("the old program ends");
    let start = finish_restart(pending).expect("the launch of the new program");
    board.apply_restart(id, start).expect("the panel restarts");

    let after = board
        .panel(id)
        .and_then(Panel::terminal)
        .and_then(Terminal::child_pid)
        .expect("the pid of the new program");
    assert_ne!(after, before, "a new program serves the restarted panel");
    assert!(
        matches!(panel_holder::probe(&home, "restarted-panel"), Err(HolderError::Busy)),
        "a holder serves the restarted panel"
    );
    assert_eq!(
        board.panel(id).and_then(Panel::terminal).and_then(Terminal::child_pid),
        Some(after),
        "the terminal of the restarted panel reads the new program"
    );
    assert!(!runs(before), "the program of the holder before the restart ended");
}

/// The end of Horizon detaches from the program of a holder and signals nothing:
/// that program is a child of its holder, and no panel of this process owns it.
#[test]
fn the_end_of_horizon_lets_the_program_of_a_holder_run() {
    let (_folder, home) = temp_home();
    let (mut board, id) = board_with_a_held_panel(&home, "kept-panel", true);
    let pid = board
        .panel(id)
        .and_then(Panel::terminal)
        .and_then(Terminal::child_pid)
        .expect("the pid of the program of the panel");

    let progress = board.begin_async_shutdown();
    // Longer than the hangup and the two steps that the end of a terminal gives a
    // program that outlives it, so a signal of this board would have landed.
    thread::sleep(crate::panel::AFTER_HANGUP + crate::panel::AFTER_TERM + Duration::from_millis(500));

    assert!(
        runs(pid),
        "the program of a holder outlives the end of Horizon (pid {pid})"
    );
    assert!(
        matches!(panel_holder::probe(&home, "kept-panel"), Err(HolderError::Busy)),
        "the holder of the panel still runs its program"
    );
    assert!(
        progress.wait_for_completion(Duration::from_secs(1)),
        "the end of Horizon counts the panel of a holder and waits for no program"
    );
}

/// The wait of a start for the holder of the program before has a deadline, and
/// the bound is a value of the call: a bound of the test ends the wait there.
#[test]
fn a_start_waits_for_the_holder_before_it_only_to_the_bound_of_its_caller() {
    let (_folder, home) = temp_home();
    panel_holder::serve_on_a_thread(&home, &stand_in("waited-panel")).expect("the holder of a test");
    let client = panel_holder::linked(&home, "waited-panel")
        .expect("the link of the holder")
        .expect("a holder answers");
    wait_until("the holder to serve another client", || {
        matches!(panel_holder::probe(&home, "waited-panel"), Err(HolderError::Busy))
    });

    // A holder answers for the panel, so the wait of the start runs to its bound.
    let bound = Duration::from_millis(300);
    let started = Instant::now();
    let (link, notice) = started_within(&home, &stand_in("waited-panel"), bound);
    let waited = started.elapsed();
    drop(client);

    assert!(
        link.is_none(),
        "no link answers while the holder of the program before stands"
    );
    assert!(
        notice
            .as_deref()
            .is_some_and(|line| line.contains("serves another client")),
        "the line of the start says why no holder serves the panel: {notice:?}"
    );
    assert!(
        waited >= bound,
        "the start waits for the holder before it until its bound: {waited:?}"
    );
    assert!(
        waited < CLEAR_BOUND,
        "the bound of the caller ends the wait, and not the five seconds of a start: {waited:?}"
    );
}
