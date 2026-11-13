//! The bounds of the end of a program that ignores the hangup of its terminal.
//!
//! These tests start a real program of this computer in a real terminal and
//! measure how long the shutdown of the board takes. The bounds are loose,
//! because a loaded computer runs the same steps more slowly; the order of the
//! steps is what they prove.

use std::time::{Duration, Instant};

use crate::board::Board;
use crate::panel::{AFTER_HANGUP, AFTER_TERM, PanelKind, PanelOptions};

/// One board with one terminal panel whose program is `/bin/sh` that runs
/// `script`.
fn board_that_runs(script: &str) -> (Board, u32) {
    let mut board = Board::new();
    let workspace = board.create_workspace("the end of a program");
    let panel = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), script.to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("the panel of the test starts");
    let pid = board
        .panel(panel)
        .and_then(crate::panel::Panel::terminal)
        .and_then(crate::terminal::Terminal::child_pid)
        .expect("the program of the panel has an id");
    (board, pid)
}

/// Waits until `pid` is gone, and answers whether it is.
fn gone_within(pid: u32, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    while Instant::now() < deadline {
        if !crate::process::is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    !crate::process::is_alive(pid)
}

/// The bound of one test, which is far above the bound of the product so a
/// loaded computer does not fail it.
const TEST_BOUND: Duration = Duration::from_secs(6);

/// E1: a program that ends at the hangup of its terminal ends at once, and the
/// shutdown of the board waits for no step of the escalation.
#[test]
fn a_program_that_ends_at_the_hangup_ends_at_once() {
    let (mut board, pid) = board_that_runs("sleep 30");
    let started = Instant::now();
    board.shutdown_terminal_panels();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEST_BOUND,
        "the shutdown of the board is bounded: {elapsed:?}"
    );
    assert!(
        elapsed < AFTER_HANGUP,
        "the program needed no step of the escalation: {elapsed:?}"
    );
    assert!(gone_within(pid, TEST_BOUND), "the program of the panel is gone");
}

/// E2: a program that ignores the hangup and `SIGTERM` ends by `SIGKILL`, and
/// the board does not wait for it without a bound.
#[test]
fn a_program_that_ignores_the_hangup_and_sigterm_ends_by_sigkill() {
    let (mut board, pid) = board_that_runs("trap '' HUP TERM; while :; do sleep 0.05; done");
    let started = Instant::now();
    board.shutdown_terminal_panels();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEST_BOUND,
        "the shutdown of the board is bounded: {elapsed:?}"
    );
    assert!(
        elapsed >= AFTER_HANGUP + AFTER_TERM,
        "the program outlived the hangup and SIGTERM: {elapsed:?}"
    );
    assert!(gone_within(pid, TEST_BOUND), "the program of the panel is gone");
}

/// E3: a program that ignores the hangup but ends on `SIGTERM` ends by the
/// first step, before the second one runs.
#[test]
fn a_program_that_ignores_only_the_hangup_ends_by_sigterm() {
    let (mut board, pid) = board_that_runs("trap '' HUP; while :; do sleep 0.05; done");
    let started = Instant::now();
    board.shutdown_terminal_panels();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TEST_BOUND,
        "the shutdown of the board is bounded: {elapsed:?}"
    );
    assert!(
        elapsed >= AFTER_HANGUP,
        "the program outlived the hangup of its terminal: {elapsed:?}"
    );
    assert!(
        elapsed < AFTER_HANGUP + AFTER_TERM,
        "SIGTERM ended the program before the second step: {elapsed:?}"
    );
    assert!(gone_within(pid, TEST_BOUND), "the program of the panel is gone");
}

/// E4: the close control of a panel takes the same steps as the exit, so a
/// program that ignores the hangup cannot run on after its panel is gone.
#[test]
fn a_closed_panel_ends_its_program_with_the_same_steps() {
    let (mut board, pid) = board_that_runs("trap '' HUP TERM; while :; do sleep 0.05; done");
    let workspace = board
        .workspaces
        .first()
        .map(|workspace| workspace.id)
        .expect("workspace");
    let panel = board
        .workspace(workspace)
        .and_then(|workspace| workspace.panels.first().copied())
        .expect("the panel of the test");
    let started = Instant::now();
    board.close_panel(panel);
    assert!(
        gone_within(pid, TEST_BOUND),
        "the program of the closed panel is gone: {elapsed:?}",
        elapsed = started.elapsed()
    );
    let elapsed = started.elapsed();
    assert!(elapsed < TEST_BOUND, "the close of the panel is bounded: {elapsed:?}");
    assert!(
        elapsed >= AFTER_HANGUP,
        "the program outlived the hangup of the closed panel: {elapsed:?}"
    );
}
