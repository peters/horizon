//! A program on a real pseudo terminal, followed through the outlet of its
//! terminal: what the outlet gives, and the order of a screen and the output
//! after it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{Screen, assert_same, replay_live_bytes, rows_text, screen};
use crate::terminal::share::{ShareOutlet, ShareScreen, ShareTake};
use crate::terminal::{Terminal, TerminalSpawnOptions};

/// The longest wait for the program of a test. It is a hang guard and not a rule
/// of the product: a loaded computer schedules the program late.
const PATIENCE: Duration = Duration::from_secs(120);

fn options(command: &str, rows: u16, cols: u16) -> TerminalSpawnOptions {
    TerminalSpawnOptions {
        program: "/bin/sh".to_owned(),
        args: vec!["-c".to_owned(), command.to_owned()],
        cwd: None,
        rows,
        cols,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 256,
        window_id: 41,
        replay_bytes: Vec::new(),
        env: HashMap::new(),
        kitty_keyboard: true,
    }
}

fn spawn(command: &str, rows: u16, cols: u16) -> Terminal {
    Terminal::spawn(options(command, rows, cols)).expect("a terminal")
}

/// Waits until the screen of `terminal` shows `text`.
fn wait_for(terminal: &mut Terminal, text: &str) {
    let deadline = Instant::now() + PATIENCE;
    while !terminal.viewport_text().iter().any(|line| line.contains(text)) {
        terminal.process_events();
        assert!(
            Instant::now() < deadline,
            "the program printed {text:?}, and the screen shows {:?}",
            terminal.viewport_text()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Takes from `outlet` until `length` bytes arrived.
fn taken(terminal: &mut Terminal, outlet: &ShareOutlet, length: usize) -> Vec<u8> {
    let deadline = Instant::now() + PATIENCE;
    let mut bytes = Vec::new();
    while bytes.len() < length {
        terminal.process_events();
        let take = outlet.take();
        assert!(!take.overflowed, "the outlet continues its screen");
        bytes.extend_from_slice(&take.bytes);
        assert!(
            Instant::now() < deadline,
            "{length} bytes arrive, and these did: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    bytes
}

/// A new follower that parsed `shared`.
fn follower_of(shared: &ShareScreen) -> Screen {
    let follower = screen(shared.rows, shared.cols);
    replay_live_bytes(&follower, &shared.bytes);
    follower
}

#[test]
fn the_outlet_of_a_terminal_is_empty_until_somebody_follows_and_then_gives_its_program_in_order() {
    // The program says that it waits with a word that ends no line, so nothing
    // of it is on its way when the screen shows that word.
    let mut terminal = spawn(
        "stty -echo; printf 'one\\nready'; read x; printf 'two\\033[31mred\\033[0m\\n'; printf 'three\\n'; exec cat",
        24,
        80,
    );
    let outlet = terminal.share_outlet();
    wait_for(&mut terminal, "ready");
    // A screen that nobody follows starts nothing.
    let _unfollowed = terminal.share_screen();
    assert_eq!(
        outlet.take(),
        ShareTake::default(),
        "no byte reaches the outlet while nobody follows"
    );

    outlet.set_wanted(true);
    assert!(outlet.take().overflowed, "a follower needs a screen first");
    let shared = terminal.share_screen();
    assert_eq!((shared.rows, shared.cols), (24, 80));
    assert_eq!(outlet.take(), ShareTake::default(), "the program wrote nothing since");

    terminal.write_input(b"\n");
    let expected = b"two\x1b[31mred\x1b[0m\r\nthree\r\n";
    let output = taken(&mut terminal, &outlet, expected.len());
    assert_eq!(output, expected, "the bytes of the program, once and in its order");

    let follower = follower_of(&shared);
    replay_live_bytes(&follower, &output);
    wait_for(&mut terminal, "three");
    assert_same(&terminal.term, &follower);
    assert_eq!(rows_text(&follower.lock())[..3], ["one", "readytwored", "three"]);
    assert!(terminal.shutdown_with_timeout(PATIENCE));
}

#[test]
fn output_that_nobody_takes_in_time_is_dropped_and_a_new_screen_starts_the_outlet_again() {
    let mut terminal = spawn(
        "stty -echo; printf 'ready'; read x; head -c 1200000 /dev/zero | tr '\\0' 'x'; \
         printf '\\nmore'; read y; printf 'after\\n'; exec cat",
        24,
        80,
    );
    let outlet = terminal.share_outlet();
    outlet.set_wanted(true);
    wait_for(&mut terminal, "ready");
    let _first = terminal.share_screen();

    terminal.write_input(b"\n");
    wait_for(&mut terminal, "more");
    assert_eq!(
        outlet.take(),
        ShareTake {
            bytes: Vec::new(),
            overflowed: true
        },
        "more than the bound waited, so the outlet holds nothing and asks for a screen"
    );

    let shared = terminal.share_screen();
    let follower = follower_of(&shared);
    terminal.write_input(b"\n");
    let output = taken(&mut terminal, &outlet, b"after\r\n".len());
    assert_eq!(output, b"after\r\n");
    replay_live_bytes(&follower, &output);
    wait_for(&mut terminal, "after");
    assert_same(&terminal.term, &follower);
    assert!(terminal.shutdown_with_timeout(PATIENCE));
}

#[test]
fn a_resize_of_the_terminal_asks_for_a_new_screen_of_the_new_grid() {
    let mut terminal = spawn("printf 'ready'; exec cat", 24, 80);
    let outlet = terminal.share_outlet();
    outlet.set_wanted(true);
    wait_for(&mut terminal, "ready");
    let _first = terminal.share_screen();
    assert!(!outlet.take().overflowed);

    terminal.resize(24, 80, 9, 18);
    assert!(!outlet.take().overflowed, "larger cells leave the grid as it is");
    terminal.resize(30, 100, 9, 18);

    assert!(outlet.take().overflowed, "the screen of the old grid is over");
    let shared = terminal.share_screen();
    assert_eq!((shared.rows, shared.cols), (30, 100));
    assert!(!outlet.take().overflowed);
    assert_same(&terminal.term, &follower_of(&shared));
    assert!(terminal.shutdown_with_timeout(PATIENCE));
}

#[test]
fn a_follower_of_a_snapshot_gets_its_screen_and_none_of_the_output_of_its_process() {
    let mut options = TerminalSpawnOptions {
        replay_bytes: b"Saved screen\r\n".to_vec(),
        ..options("sleep 0.2; printf 'SCRATCH'", 24, 80)
    };
    options.kitty_keyboard = false;
    let mut terminal = Terminal::spawn_snapshot(options).expect("a snapshot");
    let outlet = terminal.share_outlet();
    outlet.set_wanted(true);
    let shared = terminal.share_screen();

    let deadline = Instant::now() + PATIENCE;
    while !terminal.child_exited() {
        terminal.process_events();
        assert!(Instant::now() < deadline, "the process of the snapshot ended");
        std::thread::sleep(Duration::from_millis(2));
    }

    assert_eq!(outlet.take(), ShareTake::default(), "the process is not shown");
    assert_eq!(rows_text(&follower_of(&shared).lock())[0], "Saved screen");
}

/// The lines that the program of the order test writes: the numbers from one to
/// `LINES`, and a last line.
const LINES: usize = 500_000;
const LAST: &str = "end";

/// The bytes that the program of the order test writes in all, and the place in
/// them of each line and of their end.
fn numbered_stream() -> (Vec<u8>, Vec<usize>) {
    let mut stream = Vec::new();
    let mut starts = Vec::new();
    for line in 1..=LINES {
        starts.push(stream.len());
        stream.extend_from_slice(format!("{line}\r\n").as_bytes());
    }
    starts.push(stream.len());
    stream.extend_from_slice(format!("{LAST}\r\n").as_bytes());
    starts.push(stream.len());
    (stream, starts)
}

/// How many bytes of the numbered stream `follower` has drawn, read from what it
/// shows: the line of the row above its cursor, and the part of the next line
/// that stands on the row of its cursor.
fn drawn_bytes(follower: &Screen, starts: &[usize]) -> usize {
    let term = follower.lock();
    let cursor = term.grid().cursor.point;
    let rows = rows_text(&term);
    let row = usize::try_from(cursor.line.0).expect("the row of the cursor");
    let above = match row.checked_sub(1).map(|above| rows[above].as_str()) {
        None | Some("") => 0,
        Some(LAST) => LINES + 1,
        Some(number) => number.parse().expect("a number of the stream"),
    };
    let part = rows[row].len();
    // A cursor at the start of a row that holds text took the carriage return of
    // that line and not yet its line feed.
    starts[above]
        + if cursor.column.0 == 0 && part > 0 {
            part + 1
        } else {
            part
        }
}

/// The order rule of a screen and the output after it. A program writes numbers
/// without a pause, and a new follower joins again and again while it writes: it
/// takes a screen and then the output of the outlet. What each follower drew with
/// its screen and what it got after it must be the stream of the program byte for
/// byte, with no byte lost and none doubled at the join.
#[test]
fn the_output_after_a_screen_continues_it_with_no_gap_and_no_repeat() {
    let (stream, starts) = numbered_stream();
    let mut terminal = spawn(&format!("seq 1 {LINES}; printf '{LAST}\\n'; exec cat"), 24, 20);
    let outlet = terminal.share_outlet();
    outlet.set_wanted(true);

    let deadline = Instant::now() + PATIENCE;
    let (mut joins, mut joins_while_writing) = (0, 0);
    // The follower of now, where its screen stands in the stream, and what the
    // outlet gave after that screen.
    let mut following: Option<(Screen, usize, Vec<u8>)> = None;
    loop {
        terminal.process_events();
        let take = outlet.take();
        let mut got = 0;
        if let Some((follower, start, output)) = &mut following {
            assert!(!take.overflowed || take.bytes.is_empty());
            replay_live_bytes(follower, &take.bytes);
            output.extend_from_slice(&take.bytes);
            assert!(
                stream[*start..].starts_with(output.as_slice()),
                "the output after the screen at byte {start} continues it"
            );
            assert_eq!(
                drawn_bytes(follower, &starts),
                *start + output.len(),
                "the follower drew the stream up to the last byte that it got"
            );
            got = output.len();
            if *start + got == stream.len() {
                break;
            }
        }
        // A new follower joins when the outlet asks for a screen, and after
        // every take that brought output.
        if take.overflowed || got > 0 || following.is_none() {
            let shared = terminal.share_screen();
            let follower = follower_of(&shared);
            let start = drawn_bytes(&follower, &starts);
            joins += 1;
            joins_while_writing += usize::from(start > 0 && start < stream.len());
            following = Some((follower, start, Vec::new()));
        }
        assert!(Instant::now() < deadline, "the program wrote its last line");
    }

    let (follower, ..) = following.expect("the last follower");
    wait_for(&mut terminal, LAST);
    assert_same(&terminal.term, &follower);
    eprintln!("{joins_while_writing} of {joins} followers joined while the program wrote");
    assert!(joins_while_writing > 0, "a follower joined while the program wrote");
    assert!(terminal.shutdown_with_timeout(PATIENCE));
}
