use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{Terminal, TerminalSpawnOptions};

/// A process that does what Windows `ConPTY` does when a console starts: it names the
/// console in the title and clears the screen. On Windows, `ConPTY` does it for `cls`.
fn console_start() -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd.exe".into(), vec!["/D".into(), "/C".into(), "cls".into()])
    } else {
        (
            "/bin/sh".into(),
            vec!["-c".into(), r"printf '\033]0;console\007\033[2J\033[HSCRATCH'".into()],
        )
    }
}

fn options() -> TerminalSpawnOptions {
    let (program, args) = console_start();
    TerminalSpawnOptions {
        program,
        args,
        cwd: None,
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 256,
        window_id: 41,
        replay_bytes: b"\x1b]0;snapshot\x07Saved screen\r\n".to_vec(),
        env: HashMap::new(),
        kitty_keyboard: false,
    }
}

/// Waits until the process exited and the event loop parsed all of its output.
fn settled(mut terminal: Terminal) -> Terminal {
    terminal.release_when_exited();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !terminal.pty_released() {
        assert!(Instant::now() < deadline, "the terminal process did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    terminal.process_events();
    terminal
}

#[test]
fn a_snapshot_keeps_its_replay_whatever_its_process_prints() {
    let snapshot = settled(Terminal::spawn_snapshot(options()).expect("spawn snapshot"));

    assert_eq!(snapshot.viewport_text(), ["Saved screen"]);
    assert_eq!(snapshot.title(), "snapshot");
    assert!(snapshot.child_exited());
}

#[cfg(unix)]
#[test]
fn a_live_terminal_shows_what_its_process_prints_over_the_replay() {
    let live = settled(Terminal::spawn(options()).expect("spawn terminal"));

    assert_eq!(live.viewport_text(), ["SCRATCH"]);
    assert_eq!(live.title(), "console");
}
