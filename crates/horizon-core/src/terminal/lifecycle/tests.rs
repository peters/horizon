use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::{Ordering, Terminal, TerminalSpawnOptions};

struct ChildRelease(PathBuf);

impl ChildRelease {
    fn release(&self) {
        std::fs::write(&self.0, b"release").expect("release owned fixture child");
    }
}

impl Drop for ChildRelease {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"release");
    }
}

fn wait_until(mut ready: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn dropping_a_finished_event_loop_does_not_wait_for_its_live_child() {
    let state = tempfile::tempdir().expect("owned PTY fixture state");
    let mut terminal = Terminal::spawn(TerminalSpawnOptions {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "trap '' HUP; printf ready > ready; while [ ! -e release ]; do sleep 0.01; done".into(),
        ],
        cwd: Some(state.path().to_path_buf()),
        rows: 16,
        cols: 60,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 256,
        window_id: 41,
        replay_bytes: Vec::new(),
        env: HashMap::new(),
        kitty_keyboard: true,
    })
    .expect("spawn owned PTY fixture");
    // Release before Terminal during unwinding, including an old-code failure.
    let release = ChildRelease(state.path().join("release"));
    wait_until(
        || state.path().join("ready").exists(),
        "child did not install its SIGHUP trap",
    );
    terminal.request_shutdown();
    wait_until(
        || {
            terminal
                .event_loop_handle
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
        },
        "event loop did not finish before terminal drop",
    );
    assert!(
        !release.0.exists(),
        "the finished event loop still owns a live gated child"
    );
    let complete = Arc::clone(&terminal.shutdown_complete);
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        drop(terminal);
        dropped_tx.send(()).expect("drop observer remains available");
    });
    let dropped_before_release = dropped_rx.recv_timeout(Duration::from_secs(2)).is_ok();
    // Unblock the old implementation before joining or asserting its failure.
    release.release();
    if !dropped_before_release {
        dropped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("drop caller finishes");
    }
    caller.join().expect("terminal drop caller did not panic");
    assert!(
        dropped_before_release,
        "terminal drop waited for its still-running PTY child"
    );
    wait_until(
        || complete.load(Ordering::Acquire),
        "background PTY cleanup did not complete",
    );
}
