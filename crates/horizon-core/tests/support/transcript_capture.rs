#![cfg(not(windows))]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use horizon_core::{Board, PanelId, PanelKind, PanelOptions};
use uuid::Uuid;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn run_self(test_name: &str, path_override: Option<OsString>) {
    let mut command = Command::new(std::env::current_exe().expect("current test binary"));
    command.args(["--ignored", "--exact", test_name, "--nocapture", "--test-threads=1"]);

    if let Some(path) = path_override {
        command.env("PATH", path);
    } else {
        command.env_remove("PATH");
    }

    let output = command.output().expect("spawn child test process");
    assert!(
        output.status.success(),
        "child test `{test_name}` failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

pub fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("horizon-transcript-e2e-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).expect("temp root");
    root
}

pub fn assert_shell_and_command_panels_start_without_capture() {
    let root = temp_root("panels");
    let transcript_root = root.join("transcripts");
    let mut board = Board::new();
    let workspace_id = board.create_workspace("transcript-e2e");

    let shell_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "printf 'shell-ready\\n'; exec /bin/sh".to_string()],
                transcript_root: Some(transcript_root.clone()),
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("shell panel should spawn without transcript capture");
    let command_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "printf 'command-ready\\n'; exec /bin/sh".to_string()],
                transcript_root: Some(transcript_root.clone()),
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("command panel should spawn without transcript capture");

    wait_for_marker(&mut board, shell_id, "shell-ready");
    wait_for_marker(&mut board, command_id, "command-ready");
    assert_capture_wrapper_did_not_start(&board, &transcript_root, shell_id);
    assert_capture_wrapper_did_not_start(&board, &transcript_root, command_id);

    board.shutdown_terminal_panels();
    fs::remove_dir_all(root).ok();
}

fn wait_for_marker(board: &mut Board, panel_id: PanelId, marker: &str) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        board.process_output();
        let panel = board.panel(panel_id).expect("panel should exist");
        let terminal = panel.terminal().expect("terminal panel");
        let snapshot = terminal.last_lines_text(16);
        if snapshot.contains(marker) {
            assert!(
                !panel.child_exited(),
                "panel exited after emitting `{marker}`:\n{snapshot}"
            );
            return;
        }
        assert!(
            !panel.child_exited(),
            "panel exited before emitting `{marker}`:\n{snapshot}"
        );
        thread::sleep(POLL_INTERVAL);
    }

    let snapshot = board
        .panel(panel_id)
        .and_then(|panel| panel.terminal())
        .map(|terminal| terminal.last_lines_text(16))
        .unwrap_or_default();
    panic!("timed out waiting for `{marker}`. Last terminal lines:\n{snapshot}");
}

fn assert_capture_wrapper_did_not_start(board: &Board, transcript_root: &Path, panel_id: PanelId) {
    let panel = board.panel(panel_id).expect("panel should exist");
    let session_path = transcript_root.join(format!("{}.session", panel.local_id));
    assert!(
        !session_path.exists(),
        "transcript wrapper unexpectedly created {}",
        session_path.display()
    );
}
