//! A terminal of a panel that attaches to the holder of its program.

use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use super::{Paths, Serving, TEST_BOUND, attach_when_free, close, launch, temp_home};
use crate::HorizonHome;
use crate::terminal::{Terminal, TerminalSpawnOptions};
use horizon_pty_tap::frames::{self, Frame, Hello};

/// The options of the terminal of a test: the holder already runs its program.
fn options() -> TerminalSpawnOptions {
    TerminalSpawnOptions {
        program: String::new(),
        args: Vec::new(),
        cwd: None,
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 256,
        window_id: 7,
        replay_bytes: Vec::new(),
        env: HashMap::new(),
        kitty_keyboard: true,
    }
}

/// Attaches a terminal to the holder of `local_id`, while the link is free.
fn attach(home: &HorizonHome, local_id: &str) -> Terminal {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        if let Ok(Some(link)) = crate::panel_holder::linked(home, local_id) {
            match Terminal::spawn_attached(options(), link.socket, link.hello, link.replay) {
                Ok(terminal) => return terminal,
                Err(error) => assert!(Instant::now() < deadline, "a terminal attaches: {error}"),
            }
        } else {
            assert!(Instant::now() < deadline, "a holder answers for {local_id}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until the grid of `terminal` holds `wanted`, and answers its text.
fn wait_for_text(terminal: &mut Terminal, wanted: &str) -> String {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        terminal.process_events();
        let text = terminal.try_last_lines_text(40).unwrap_or_default();
        if text.contains(wanted) {
            return text;
        }
        assert!(Instant::now() < deadline, "waited for {wanted:?}, read {text:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until `read` answers a value of the terminal, and fails the test when it
/// does not.
fn wait_for<T>(terminal: &mut Terminal, what: &str, mut read: impl FnMut(&Terminal) -> Option<T>) -> T {
    let deadline = Instant::now() + TEST_BOUND;
    loop {
        terminal.process_events();
        if let Some(value) = read(terminal) {
            return value;
        }
        assert!(Instant::now() < deadline, "waited for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_text_and_the_input_of_an_attached_terminal_cross_the_holder() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            "attached-input",
            "printf 'held text\\n'; read line; printf 'read %s\\n' \"$line\"",
        ),
        1024,
    );
    let mut terminal = attach(&home, "attached-input");

    wait_for_text(&mut terminal, "held text");
    terminal.write_input(b"ping\n");
    let text = wait_for_text(&mut terminal, "read ping");
    assert!(
        text.contains("read ping"),
        "the input of the terminal reached the program: {text:?}"
    );

    drop(terminal);
    serving.ended();
}

#[test]
fn a_resize_of_an_attached_terminal_reaches_the_program() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch("attached-resize", "read line; stty size; exec cat"),
        1024,
    );
    let mut terminal = attach(&home, "attached-resize");

    terminal.resize(40, 100, 9, 18);
    terminal.write_input(b"go\n");
    wait_for_text(&mut terminal, "40 100");

    drop(terminal);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    close(&mut client, serving);
}

#[test]
fn a_second_attached_terminal_finds_the_same_program_and_its_text() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            "attached-again",
            "printf 'before the detach\\n'; sleep 1; printf 'after the detach\\n'; exec cat",
        ),
        1024,
    );
    let mut first = attach(&home, "attached-again");
    wait_for_text(&mut first, "before the detach");
    let pid = first.child_pid().expect("the pid of the program of the holder");
    drop(first);

    let mut second = attach(&home, "attached-again");
    assert_eq!(
        second.child_pid(),
        Some(pid),
        "both terminals read the program of one holder"
    );
    let text = wait_for_text(&mut second, "after the detach");
    assert!(
        text.contains("before the detach"),
        "the replay of the holder shows the text from before the detach: {text:?}"
    );

    drop(second);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    close(&mut client, serving);
}

#[test]
fn the_end_of_the_program_of_a_holder_reaches_the_terminal_with_its_status() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(&home, &launch("attached-end", "read line; exit 3"), 1024);
    let mut terminal = attach(&home, "attached-end");

    terminal.write_input(b"end\n");
    let status = wait_for(&mut terminal, "the end of the program", |terminal| {
        terminal.child_exit_status().map(|status| status.code())
    });
    assert_eq!(status, Some(3), "the terminal reads the status of the program");
    assert!(terminal.child_exited(), "the terminal reports the end of its program");

    drop(terminal);
    serving.ended();
}

#[test]
fn a_status_report_of_a_held_program_reaches_the_inbox_of_the_terminal() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            "attached-status",
            "read line; printf '\\033]7501;state=working\\033\\\\'; printf 'the status went out\\n'; exec cat",
        ),
        1024,
    );
    let mut terminal = attach(&home, "attached-status");

    terminal.write_input(b"go\n");
    wait_for_text(&mut terminal, "the status went out");
    let report = wait_for(&mut terminal, "the program status of the program", |terminal| {
        terminal
            .take_program_payload()
            .map(|payload| payload.as_bytes().to_vec())
    });
    assert_eq!(report, b"state=working", "the same inbox as a local program fills");

    drop(terminal);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    close(&mut client, serving);
}

#[test]
fn the_end_of_a_holder_with_no_exit_frame_ends_the_terminal_without_spinning() {
    let (_folder, home) = temp_home();
    let asked = launch("attached-lost", "printf 'never started'");
    let socket = Paths::of(&home, &asked.local_id).expect("the socket of the holder");
    socket.prepare().expect("the folder of the sockets");
    let listener = socket.bind().expect("the socket of a holder of a test");

    let holder = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the terminal of a test");
        let hello = Hello {
            pid: 1,
            start_time: None,
            ring_len: 9,
        };
        frames::write(&mut stream, &Frame::Hello(hello)).expect("the hello of the holder");
        frames::write(&mut stream, &Frame::Replay(b"last text".to_vec())).expect("the replay of the holder");
        // This holder goes away with no exit frame, which is a holder that ended.
        drop(stream);
        drop(listener);
    });

    let mut terminal = attach(&home, "attached-lost");
    let text = wait_for_text(&mut terminal, "last text");
    assert!(
        text.contains("last text"),
        "the replay of the holder reached the grid: {text:?}"
    );
    wait_for(&mut terminal, "the end of the link", |terminal| {
        terminal.child_exited().then_some(())
    });
    assert!(
        terminal.shutdown_with_timeout(Duration::from_secs(5)),
        "the event loop of the terminal ended and does not spin"
    );
    holder.join().expect("the thread of the stand-in holder");
}

/// The ids of real panels: 36 characters, as the app gives them.
const ALTERNATE_SCREEN: &str = "2e8d5f71-9a3c-4b60-8d1e-7f4a6c0b9d25";
const REPAINTS: &str = "b4a1c6e9-0d37-4f52-9e8a-3c5d7f1b2a60";

/// A program that draws on the alternate screen and asks for the mouse still owns
/// both after an attach: the replay must not end them, as the replay of a program
/// that ended does.
#[test]
fn an_attached_terminal_keeps_the_alternate_screen_and_the_modes_of_a_live_program() {
    use alacritty_terminal::term::TermMode;

    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            ALTERNATE_SCREEN,
            "printf '\\033[?1049h\\033[?1000h\\033[2J\\033[3;5Hthe alternate screen'; exec cat",
        ),
        4096,
    );
    let mut first = attach(&home, ALTERNATE_SCREEN);
    wait_for_text(&mut first, "the alternate screen");
    drop(first);

    let second = attach(&home, ALTERNATE_SCREEN);
    let text = second.try_last_lines_text(40).unwrap_or_default();
    assert!(
        text.contains("the alternate screen"),
        "the grid after the attach shows the screen of the program: {text:?}"
    );
    assert!(
        second
            .mode()
            .contains(TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK),
        "the program still owns its screen and its mouse mode: {:?}",
        second.mode()
    );

    drop(second);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    close(&mut client, serving);
}

/// A program that draws with cursor moves repaints at a window-change signal. The
/// ring of this holder is too short for its screen, and the size of the second
/// terminal is the size of the first one, so only an attach that asks for a repaint
/// shows the screen.
#[test]
fn an_attach_asks_a_program_that_draws_with_cursor_moves_to_repaint() {
    let (_folder, home) = temp_home();
    let serving = Serving::start(
        &home,
        &launch(
            REPAINTS,
            "draw() { printf '\\033[2J\\033[3;5Hscreen of %s.' \"$(stty size)\"; }; trap draw WINCH; draw; \
             while :; do sleep 30 & wait $!; done",
        ),
        8,
    );
    let mut first = attach(&home, REPAINTS);
    wait_for_text(&mut first, "screen of 24 80.");
    drop(first);

    let mut second = attach(&home, REPAINTS);
    let text = wait_for_text(&mut second, "screen of 24 80.");
    assert!(
        text.contains("screen of 24 80."),
        "the program repainted for the terminal that attached: {text:?}"
    );

    drop(second);
    let (mut client, _hello) = attach_when_free(&serving.socket);
    close(&mut client, serving);
}
