//! The opt-in latency lines of a key press, against real PTYs.

use std::collections::HashMap;
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
