use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::editor::PanelContent;
use crate::panel::{Panel, PanelKind};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::{PanelId, PanelOptions, WorkspaceId};

use super::ProgramEnd;

/// A panel of this computer that runs `command` in a shell and ends.
fn panel_that_ends(command: &str) -> Panel {
    let terminal = Terminal::spawn(TerminalSpawnOptions {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        cwd: None,
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 256,
        window_id: 41,
        replay_bytes: Vec::new(),
        env: HashMap::new(),
        kitty_keyboard: true,
    })
    .expect("the terminal of the test starts");
    Panel::from_content(
        PanelId(1),
        WorkspaceId(1),
        PanelKind::Command,
        PanelContent::Terminal(terminal),
    )
}

/// Runs the frames of `panel` until its program ended, and answers whether it
/// did within the deadline.
fn wait_for_the_end(panel: &mut Panel) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        panel.process_output();
        if panel.program_end().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// C5: an ended program is recorded once, with the status that the system
/// reported, and later frames read the same end instead of a new one.
#[test]
fn a_program_that_ended_is_recorded_once_with_its_status() {
    let mut panel = panel_that_ends("exit 3");
    assert!(wait_for_the_end(&mut panel), "the program of the panel ends");
    assert_eq!(panel.program_end(), Some(ProgramEnd { exit_status: Some(3) }));
    assert_eq!(
        panel.program_end().map(|end| end.line()),
        Some("The program of this panel ended with status 3. Text typed here is not sent.".to_owned())
    );

    for _ in 0..3 {
        panel.process_output();
    }
    assert_eq!(
        panel.program_end(),
        Some(ProgramEnd { exit_status: Some(3) }),
        "a later frame reads the end that the panel already knows"
    );
}

/// C6: a panel whose program ended takes no key, and a panel that still runs its
/// program takes them. The line under the grid is the answer for the first one.
#[test]
fn a_panel_whose_program_ended_takes_no_key() {
    let mut ended = panel_that_ends("exit 0");
    assert!(wait_for_the_end(&mut ended), "the program of the panel ends");
    assert!(!ended.accepts_input(), "the panel holds no program");
    assert!(
        !ended.write_input(b"typed into a panel that ended\n"),
        "no program takes the key, and the panel says so"
    );

    let mut running = panel_that_ends("sleep 2");
    running.process_output();
    assert!(running.accepts_input(), "the program of the panel still runs");
    assert!(
        running.write_input(b"typed into a panel that runs\n"),
        "a running program takes the key"
    );
    running.request_shutdown();
}

/// C7: a panel whose program ended keeps its screen above the program that
/// starts again, so the person still reads what the old program wrote.
#[test]
fn a_panel_whose_program_ended_keeps_its_screen_for_the_next_program() {
    let mut panel = panel_that_ends("printf 'kept-line\\n'; exit 0");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline
        && panel
            .terminal()
            .and_then(|terminal| terminal.viewport_text().into_iter().find(|line| line == "kept-line"))
            .is_none()
    {
        panel.process_output();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(wait_for_the_end(&mut panel), "the program of the panel ends");

    let kept = String::from_utf8_lossy(&panel.restart_replay_bytes()).into_owned();
    assert!(
        kept.contains("kept-line"),
        "the screen of the ended program stays above the next one: {kept:?}"
    );
    assert!(
        kept.ends_with("\r\n"),
        "the next program starts under the text that the person read: {kept:?}"
    );
}

/// C8: a panel that runs a stand-in process for a screen draws no program of the
/// person, so its exit is not reported as an ended program.
#[test]
fn a_panel_that_shows_a_screen_reports_no_ended_program() {
    let mut panel = Panel::restore_failure(
        PanelId(2),
        WorkspaceId(1),
        PanelOptions::default(),
        "the directory of this panel is gone",
    )
    .expect("the placeholder of the test starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !panel.child_exited() {
        panel.process_output();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel.child_exited(), "the stand-in process of the screen ends");
    assert_eq!(
        panel.program_end(),
        None,
        "the exit of a stand-in process is no ended program of a person"
    );
    assert!(panel.accepts_input(), "the panel of a screen keeps its own line");
}

/// C9: an SSH panel reports a lost link and never an ended program, so its own
/// line stays the only one.
#[test]
fn an_ssh_panel_reports_its_lost_link_and_no_ended_program() {
    let mut panel = panel_that_ends("exit 0");
    panel.kind = PanelKind::Ssh;
    panel.ssh_status = Some(crate::ssh::SshConnectionStatus::Connecting);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !panel.child_exited() {
        panel.process_output();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel.child_exited(), "the ssh process of the panel ends");
    assert_eq!(
        panel.program_end(),
        None,
        "the line of the lost link is the one line of an SSH panel"
    );
    assert_eq!(
        panel.ssh_status(),
        Some(crate::ssh::SshConnectionStatus::Disconnected),
        "the panel reports the end of its ssh process instead"
    );
}
