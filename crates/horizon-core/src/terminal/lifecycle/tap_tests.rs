//! A program status and a feature detection query, from a real pseudo terminal.
//!
//! The reader of a panel hands every byte of its program's output to the VT
//! parser unchanged (proven in `horizon_pty_tap`), and it hands the payloads of the
//! program status protocol to the frame beside them. This test runs the two halves
//! together on a real pseudo terminal: a program writes text and the protocol
//! sequences, and the terminal shows the text of its screen and the report of its
//! program. A second program asks the terminal whether it knows the protocol, and
//! reads the answer.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::PanelKind;
use crate::agent_work::{DETECTION_REPLY, ProgramStatus, program_status};
use crate::terminal::{Terminal, TerminalSpawnOptions};

/// The longest time that a test waits for the program of its panel, or for the
/// report that the protocol of that program writes. It is a hang guard and not a
/// rule of the product: a loaded computer schedules the program late.
const PATIENCE: Duration = Duration::from_secs(120);

fn options(command: String) -> TerminalSpawnOptions {
    TerminalSpawnOptions {
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), command],
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
    }
}

/// The payloads that the program of `terminal` reported, in the order that the
/// frame read them, with the screen rows of the terminal after them.
fn reports(mut terminal: Terminal, expected: &[ProgramStatus]) -> (Vec<ProgramStatus>, Vec<String>, String) {
    let deadline = Instant::now() + PATIENCE;
    let mut found: Vec<ProgramStatus> = Vec::new();
    while found.len() < expected.len() {
        terminal.process_events();
        if let Some(payload) = terminal.take_program_payload() {
            let status = program_status(PanelKind::Codex, payload.as_bytes());
            // The report of a state that the claim already holds is no new state,
            // so the reports arrive as the states of the panel change.
            if found.last() != Some(&status) {
                found.push(status);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the program of the panel reported {expected:?}, and {found:?} arrived"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    (found, terminal.viewport_text(), terminal.title().to_owned())
}

#[test]
fn the_reports_of_a_real_program_arrive_and_the_screen_holds_its_text() {
    // The program paces its reports the way a turn takes seconds, so each one has a
    // frame of its own to arrive in; the one report of a level protocol replaces
    // the record of the panel whole. One report arrives before each line of text,
    // so the two paths are read together, and the program keeps its terminal open
    // so the screen is read while it runs.
    let command = [
        "printf '%s' 'one\r\n'".to_owned(),
        "printf '\\033]7501;state=working:app=codex\\033\\\\'".to_owned(),
        "printf '\\033]0;work\\007'".to_owned(),
        "printf '%s' 'two\r\n'".to_owned(),
        "sleep 0.3".to_owned(),
        "printf '\\033]7501;state=blocked:app=codex:kind=permission\\033\\\\'".to_owned(),
        "printf '%s' 'three\r\n'".to_owned(),
        "sleep 0.3".to_owned(),
        "printf '\\033]7501;state=done:app=codex\\033\\\\'".to_owned(),
        "exec cat".to_owned(),
    ]
    .join("; ");

    let (found, text, title) = reports(
        Terminal::spawn(options(command)).expect("a terminal"),
        &[ProgramStatus::Working, ProgramStatus::Blocked, ProgramStatus::Done],
    );

    assert_eq!(text, ["one", "two", "three"], "the grid shows the text and no sequence");
    assert_eq!(title, "work", "the title of the program still reaches the panel");
    assert_eq!(found.len(), 3);
}

#[test]
fn a_program_that_asks_for_the_protocol_reads_the_reply() {
    let temp = tempfile::tempdir().expect("the folder of the answer");
    let answer = temp.path().join("answer.bin");
    // The reply of the feature detection query ends in no newline, so the program
    // reads its standard input byte by byte: a terminal in its default line mode
    // would hold the reply back until a line ends.
    let command = format!(
        "stty raw -echo; printf '\\033]7501;?\\033\\\\'; exec dd bs=1 count=80 of='{}' 2>/dev/null",
        answer.display()
    );
    let mut terminal = Terminal::spawn(options(command)).expect("a terminal");

    let deadline = Instant::now() + PATIENCE;
    loop {
        terminal.process_events();
        if std::fs::read(&answer).is_ok_and(|bytes| bytes == DETECTION_REPLY) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the program read the reply of the feature detection query, and read {:?}",
            std::fs::read(&answer).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        terminal.take_program_payload().is_none(),
        "the query is no report of a state"
    );
}
