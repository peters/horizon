use std::time::Duration;

use crate::attention::{AttentionSeverity, AttentionState};
use crate::panel::{PanelKind, PanelOptions};

use super::super::*;
use super::editor_panel_options;

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
fn close_panel_removes_panel_attention() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = PanelId(7);

    board.create_attention(
        workspace_id,
        Some(panel_id),
        "codex-ui",
        "Needs user feedback",
        AttentionSeverity::High,
    );

    board.close_panel(panel_id);

    assert!(board.unresolved_attention().next().is_none());
}

#[test]
fn resolve_attention_marks_item_resolved() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let attention_id = board.create_attention(
        workspace_id,
        None,
        "system",
        "Review build result",
        AttentionSeverity::Medium,
    );

    assert!(board.resolve_attention(attention_id));
    assert!(board.unresolved_attention().next().is_none());
}

#[test]
fn dismissing_attention_keeps_same_signal_suppressed_until_it_clears() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(99);

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    let attention_id = board.unresolved_attention().next().expect("open attention").id;
    assert!(board.dismiss_attention(attention_id));

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    assert!(board.unresolved_attention().next().is_none());

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "");
    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    assert!(board.unresolved_attention().next().is_some());
}

#[test]
fn stale_ready_for_input_attention_auto_dismisses() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let attention_id = board.create_attention(
        workspace_id,
        Some(PanelId(7)),
        "agent",
        "Ready for input",
        AttentionSeverity::High,
    );

    let item = board
        .attention
        .iter_mut()
        .find(|item| item.id == attention_id)
        .expect("attention item");
    item.created_at = std::time::SystemTime::now() - READY_FOR_INPUT_AUTO_DISMISS_AFTER - Duration::from_secs(1);

    board.dismiss_expired_ready_attention(READY_FOR_INPUT_AUTO_DISMISS_AFTER);

    let item = board
        .attention
        .iter()
        .find(|item| item.id == attention_id)
        .expect("attention item");
    assert_eq!(item.state, AttentionState::Dismissed);
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
        .create_panel(PanelOptions::default(), workspace_id)
        .expect("shell panel should spawn");
    let command_panel = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                ..PanelOptions::default()
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
