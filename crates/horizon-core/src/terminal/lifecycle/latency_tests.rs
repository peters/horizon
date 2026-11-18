//! The opt-in latency lines and the wake of a person's echo, against real PTYs.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::{Terminal, TerminalSpawnOptions};
use crate::terminal::latency;

fn spawn(program: &str, args: &[&str]) -> Terminal {
    Terminal::spawn(TerminalSpawnOptions {
        program: program.into(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
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
    .expect("terminal should spawn")
}

/// Drains the terminal until its process answers, so the echo of the write came back.
fn drain_until_output(terminal: &mut Terminal) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !terminal.process_events() {
        assert!(
            Instant::now() < deadline,
            "the process of the panel must answer a write"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn number_field(line: &str, name: &str) -> u64 {
    line.split(' ')
        .find_map(|field| field.strip_prefix(&format!("{name}=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("the line names {name}: {line}"))
}

fn field_names(line: &str) -> Vec<&str> {
    line.split(' ')
        .filter_map(|field| field.split_once('=').map(|(name, _)| name))
        .collect()
}

#[test]
fn a_key_press_of_a_local_shell_writes_its_line_in_the_order_of_the_hops() {
    let mut terminal = spawn("/bin/cat", &[]);
    terminal.latency.force(true);

    let key_at = terminal
        .press_clock()
        .expect("the forced switch gives the clock of the press");
    terminal.note_key_press(key_at, 1);
    terminal.write_input(b"x");
    drain_until_output(&mut terminal);
    terminal.note_output_painted();

    let lines = terminal.latency.take_lines();
    assert_eq!(lines.len(), 1, "one key press writes one line: {lines:?}");
    assert!(lines[0].starts_with("latency key seq=1 bytes=1 "), "{}", lines[0]);
    assert_eq!(
        field_names(&lines[0]),
        [
            "seq",
            "bytes",
            "event_to_write_us",
            "write_to_read_us",
            "read_to_request_us",
            "request_to_paint_us",
            "total_us",
        ]
    );

    let hops = [
        number_field(&lines[0], "event_to_write_us"),
        number_field(&lines[0], "write_to_read_us"),
        number_field(&lines[0], "read_to_request_us"),
        number_field(&lines[0], "request_to_paint_us"),
    ];
    let total = number_field(&lines[0], "total_us");
    assert!(
        hops.iter().sum::<u64>() <= total && total - hops.iter().sum::<u64>() <= 3,
        "the four hops add up to the total: {hops:?} against {total}"
    );
}

#[test]
fn a_key_press_writes_no_line_while_its_switch_is_off() {
    assert!(!latency::enabled(), "the switch of this run is off");
    let mut terminal = spawn("/bin/cat", &[]);

    let key_at = terminal.press_clock();
    terminal.note_key_press(Instant::now(), 1);
    terminal.write_input(b"x");
    drain_until_output(&mut terminal);
    terminal.note_output_painted();

    assert_eq!(key_at, None, "a frame takes no clock while the switch is off");
    assert!(
        terminal.latency.take_lines().is_empty(),
        "a key press writes no line while the switch is off"
    );
}

fn counting_wake() -> (Arc<AtomicUsize>, crate::OutputWake) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    // The frames that an echo asks for. An event of the program asks for a pass
    // of a window that draws nothing, which is not what these tests count.
    let wake: crate::OutputWake = Arc::new(move |wake| {
        counter.fetch_add(usize::from(wake == crate::Wake::Echo), Ordering::Relaxed);
    });
    (wakes, wake)
}

#[test]
fn the_echo_of_a_key_press_asks_for_one_frame() {
    let mut terminal = spawn("/bin/cat", &[]);
    let (wakes, wake) = counting_wake();
    terminal.attach_output_wake(&wake);
    // The frames of the window draw this panel, so its echo may ask for one.
    terminal.note_drawn(true);

    terminal.arm_output_wake();
    terminal.write_input(b"x");
    drain_until_output(&mut terminal);

    assert_eq!(wakes.load(Ordering::Relaxed), 1, "the echo of a key asks for its frame");
    terminal.write_input(b"y");
    drain_until_output(&mut terminal);
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        1,
        "output that no key press of a person waits for asks for no frame"
    );
}

#[test]
fn a_panel_that_prints_all_the_time_keeps_the_output_poll_and_asks_for_no_frame() {
    // The bound of a panel that nobody types into is the 16 ms output poll of
    // `finalize_frame`. A wake of its own would break that bound.
    let mut terminal = spawn("/bin/sh", &["-c", "while :; do printf 'row\\n'; sleep 0.005; done"]);
    let (wakes, wake) = counting_wake();
    terminal.attach_output_wake(&wake);
    // The panel is on screen and drawn by the frames of the window, so only the
    // missing key press can keep its output from asking for a frame.
    terminal.note_drawn(true);

    let deadline = Instant::now() + Duration::from_millis(500);
    let mut drains = 0_usize;
    while Instant::now() < deadline {
        if terminal.process_events() {
            drains += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert!(drains > 10, "the panel printed throughout the window: {drains}");
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        0,
        "output that no key press of a person waits for asks for no frame"
    );
}

/// Counts the events of a program that ask for a window that draws no frame.
fn counting_event_wake() -> (Arc<AtomicUsize>, crate::OutputWake) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let wake: crate::OutputWake = Arc::new(move |wake| {
        counter.fetch_add(usize::from(wake == crate::Wake::Event), Ordering::Relaxed);
    });
    (wakes, wake)
}

fn waits(done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

/// A program that sends, one for each line that it reads: plain output, a
/// question for the position of the cursor, a title, a status report and a
/// query of the status protocol. The answer of the terminal to a question
/// arrives in front of the next line, so each pattern takes any start.
const SENDS: &str = r"stty -echo; printf 'READY'; while read -r line; do case $line in
*plain) printf ' PLAIN';;
*ask) printf '\033[6n';;
*title) printf '\033]0;built\007';;
*status) printf '\033]7501;state=working:app=codex\033\\';;
*query) printf '\033]7501;?\033\\';;
esac; done";

#[test]
fn each_event_of_a_program_asks_one_time_for_a_window_that_draws_no_frame() {
    let mut terminal = spawn("/bin/sh", &["-c", SENDS]);
    let (wakes, wake) = counting_event_wake();
    let count = || wakes.load(Ordering::Relaxed);
    terminal.attach_output_wake(&wake);
    let shows = |terminal: &Terminal, words: &str| terminal.last_lines_text(4).contains(words);
    assert!(waits(|| shows(&terminal, "READY")), "the program is ready");
    // The program waits for a line now, so the read below takes its last event.
    std::thread::sleep(Duration::from_millis(100));
    terminal.process_events();
    assert!(!terminal.has_unread_event(), "the window read the start of the program");

    for (word, what) in [
        ("plain", "plain output"),
        ("ask", "a question"),
        ("title", "a title"),
        ("status", "a status report"),
        ("query", "a query of the status protocol"),
    ] {
        let before = count();
        terminal.write_input(format!("{word}\n").as_bytes());
        assert!(waits(|| count() == before + 1), "{what} asks for the window");
        assert!(terminal.has_unread_event(), "{what} waits for the window");
        // An event that arrives before the window read the first one joins it.
        terminal.write_input(b"title\n");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(count(), before + 1, "the events that wait ask one time");
        terminal.process_events();
        assert!(!terminal.has_unread_event(), "the window read {what}");
    }
    assert_eq!(terminal.title(), "built");
}

#[test]
fn an_event_that_arrived_before_the_wake_asks_when_the_wake_arrives() {
    let mut terminal = spawn("/bin/sh", &["-c", r"printf '\033]0;early\007'; sleep 60"]);
    assert!(waits(|| terminal.has_unread_event()), "the title waits for the window");
    let (wakes, wake) = counting_event_wake();
    terminal.attach_output_wake(&wake);
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        1,
        "the wake hears of the event that waited"
    );
    terminal.process_events();
    assert_eq!(terminal.title(), "early");
}
