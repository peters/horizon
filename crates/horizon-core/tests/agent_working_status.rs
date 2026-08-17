#![cfg(not(windows))]

use std::thread;
use std::time::{Duration, Instant};

use horizon_core::{AgentStatus, Board, Panel, PanelId, PanelKind, PanelOptions};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const WAIT_TIMEOUT: Duration = Duration::from_secs(15);
/// Comfortably past the 2s stale-working window.
const STALE_GRACE: Duration = Duration::from_millis(2500);

/// Fake agent TUI built from plain newlines only (cursor moves like `\r` get
/// mangled by the PTY line discipline). Each state is written to a fresh
/// bottom line and previous states are pushed out of the 6-row bottom scan
/// window with filler lines — a real TUI repaints in place, but scrolling
/// exercises the same detection path without depending on terminal erase
/// sequences. Octal escapes keep the braille spinner byte portable (POSIX
/// printf has no `\u`), and the filler loops use POSIX shell arithmetic so
/// the script needs no utilities (`seq`) beyond a stock `/bin/sh`.
fn fake_agent_script() -> &'static str {
    concat!(
        "fill() { n=$1; i=0; while [ $i -lt $n ]; do i=$((i+1)); echo filler-$i; done; }\n",
        "fill 18\n",
        "printf '\\342\\240\\213 Working... (esc to interrupt)'\n",
        "sleep 1.2\n",
        "fill 12\n",
        "printf 'ready: at prompt'\n",
        "sleep 1.2\n",
        "fill 12\n",
        "printf '\\342\\240\\213 Compacting context...'\n",
        "sleep 300\n",
    )
}

fn agent_status(board: &Board, panel_id: PanelId) -> AgentStatus {
    board.panel(panel_id).map_or(AgentStatus::Idle, Panel::agent_status)
}

fn screen_text(board: &Board, panel_id: PanelId) -> String {
    board
        .panel(panel_id)
        .and_then(|panel| panel.terminal())
        .map_or(String::new(), |terminal| terminal.last_lines_text(24))
}

fn wait_for_status(board: &mut Board, panel_id: PanelId, expected: AgentStatus) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while Instant::now() < deadline {
        board.process_output();
        if agent_status(board, panel_id) == expected {
            return;
        }
        let panel = board
            .panel(panel_id)
            .unwrap_or_else(|| panic!("panel {panel_id:?} should exist"));
        assert!(
            !panel.child_exited(),
            "agent panel exited before status reached {expected:?}"
        );
        thread::sleep(POLL_INTERVAL);
    }
    panic!(
        "expected agent status {expected:?}, got {:?}\n--- terminal ---\n{}",
        agent_status(board, panel_id),
        screen_text(board, panel_id)
    );
}

#[test]
fn agent_working_status_tracks_working_indicator() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("working");
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Pi,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), fake_agent_script().to_string()],
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("panel should spawn");

    // Working line appears at the bottom of the screen -> Working.
    wait_for_status(&mut board, panel_id, AgentStatus::Working);

    // The TUI scrolls the working line out of the bottom window and shows its
    // prompt (well before the 2s stale window elapses) -> detection must flip
    // the status back to Idle.
    wait_for_status(&mut board, panel_id, AgentStatus::Idle);

    // A new turn starts -> Working again.
    wait_for_status(&mut board, panel_id, AgentStatus::Working);

    // The script now stays silent: the working flag must clear on its own
    // once the terminal output goes stale.
    thread::sleep(STALE_GRACE);
    board.process_output();
    assert_eq!(
        agent_status(&board, panel_id),
        AgentStatus::Idle,
        "stale working status should reset to idle"
    );

    board.shutdown_terminal_panels();
}

#[test]
fn shell_panels_never_report_working() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("shells");
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    concat!(
                        "printf '\\342\\240\\213 Working... (esc to interrupt)'\n",
                        "sleep 300\n",
                    )
                    .to_string(),
                ],
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("shell panel should spawn");

    // Let the line reach the terminal, then confirm a non-agent panel stays
    // Idle even though the working pattern is visible.
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        board.process_output();
        if screen_text(&board, panel_id).contains("Working...") {
            break;
        }
        assert!(Instant::now() < deadline, "shell line never appeared");
        thread::sleep(POLL_INTERVAL);
    }
    board.process_output();
    assert_eq!(agent_status(&board, panel_id), AgentStatus::Idle);

    board.shutdown_terminal_panels();
}
