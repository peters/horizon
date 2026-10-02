use std::time::Duration;

use crate::panel::{PanelKind, PanelOptions};

use super::super::*;
use super::{editor_panel_options, shell_panel_options};

#[test]
fn rename_workspace_updates_matching_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");

    assert!(board.rename_workspace(workspace_id, "backend"));
    assert_eq!(board.workspaces[0].name, "backend");
}

#[test]
fn rename_workspace_rejects_blank_names() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");

    assert!(!board.rename_workspace(workspace_id, "   "));
    assert_eq!(board.workspaces[0].name, "frontend");
}

#[test]
fn rename_panel_updates_matching_panel() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    assert!(board.rename_panel(panel_id, "backend shell"));
    assert_eq!(
        board.panel(panel_id).expect("panel should exist").title,
        "backend shell"
    );
}

#[test]
fn rename_panel_rejects_blank_names() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    let original_title = board.panel(panel_id).expect("panel should exist").title.clone();

    assert!(!board.rename_panel(panel_id, "   "));
    assert_eq!(board.panel(panel_id).expect("panel should exist").title, original_title);
}

#[test]
fn global_shutdown_inherits_browser_teardown_from_an_already_closed_panel() {
    let mut board = Board::new();
    let root = tempfile::tempdir().expect("temp dir");
    let profile_dir = root.path().join("retired-browser-profile");
    std::fs::create_dir(&profile_dir).expect("profile dir");
    std::fs::write(profile_dir.join("Preferences"), b"state").expect("profile state");
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();
    board
        .retired_browser_shutdown_signals
        .push(crate::browser::BrowserShutdownSignal::for_test(completion_rx).with_profile_cleanup(profile_dir.clone()));

    let progress = board.begin_async_shutdown();

    assert_eq!(progress.panel_count(), 1);
    assert!(!progress.browser_shutdown_is_complete());
    assert!(completion_tx.send(()).is_ok());
    assert!(progress.wait_for_browser_shutdown(Duration::from_secs(1)));
    assert!(progress.is_complete());
    assert!(!profile_dir.exists());
}

#[test]
fn retired_browser_cleanup_remains_pollable_after_the_last_panel_closes() {
    let mut board = Board::new();
    let root = tempfile::tempdir().expect("temp dir");
    let profile_dir = root.path().join("last-browser-profile");
    std::fs::create_dir(&profile_dir).expect("profile dir");
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();
    board
        .retired_browser_shutdown_signals
        .push(crate::browser::BrowserShutdownSignal::for_test(completion_rx).with_profile_cleanup(profile_dir.clone()));

    assert!(board.panels.is_empty());
    assert!(board.has_pending_browser_cleanup());
    let _ = board.process_output();
    assert!(board.has_pending_browser_cleanup());
    assert!(completion_tx.send(()).is_ok());

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while board.has_pending_browser_cleanup() && std::time::Instant::now() < deadline {
        let _ = board.process_output();
        std::thread::yield_now();
    }

    assert!(!board.has_pending_browser_cleanup());
}

#[test]
fn focusing_panel_tracks_active_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    board.focus(panel_id);

    assert_eq!(board.active_workspace, Some(workspace_id));
}

#[test]
fn shutdown_terminal_panels_waits_for_shell_and_command_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("shutdown");
    let shell_panel = board
        .create_panel(shell_panel_options(), workspace_id)
        .expect("shell panel should spawn");
    let command_panel = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                ..shell_panel_options()
            },
            workspace_id,
        )
        .expect("command panel should spawn");

    board.shutdown_terminal_panels();

    assert!(
        board
            .panel_mut(shell_panel)
            .expect("shell panel should exist")
            .wait_for_shutdown(Duration::from_millis(10))
    );
    assert!(
        board
            .panel_mut(command_panel)
            .expect("command panel should exist")
            .wait_for_shutdown(Duration::from_millis(10))
    );
}

#[test]
fn begin_async_shutdown_completes_for_shell_and_command_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("shutdown");
    board
        .create_panel(shell_panel_options(), workspace_id)
        .expect("shell panel should spawn");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                ..shell_panel_options()
            },
            workspace_id,
        )
        .expect("command panel should spawn");

    let progress = board.begin_async_shutdown();
    let started_at = std::time::Instant::now();
    while !progress.is_complete() && started_at.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(progress.is_complete());
}

#[test]
fn terminal_clipboard_copies_are_collected_without_new_output() {
    use alacritty_terminal::event::Event;
    use alacritty_terminal::term::ClipboardType;

    use crate::terminal::{ClipboardTarget, ClipboardWrite};

    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(shell_panel_options(), workspace_id)
        .expect("panel should spawn");
    board
        .panel_mut(panel_id)
        .and_then(Panel::terminal_mut)
        .expect("terminal panel")
        .handle_event(Event::ClipboardStore(ClipboardType::Clipboard, "copied".to_string()));

    board.process_output();

    assert_eq!(
        board.take_terminal_clipboard_writes(),
        vec![ClipboardWrite {
            target: ClipboardTarget::Clipboard,
            text: "copied".to_string()
        }]
    );
    board.process_output();
    assert!(board.take_terminal_clipboard_writes().is_empty());
}
