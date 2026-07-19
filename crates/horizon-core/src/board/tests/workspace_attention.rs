use super::super::*;
use super::editor_panel_options;

#[test]
fn assign_panel_moves_open_and_resolved_attention_to_target_workspace() {
    let mut board = Board::new();
    let source_workspace = board.create_workspace("source");
    let target_workspace = board.create_workspace("target");
    let panel_id = board
        .create_panel(editor_panel_options(), source_workspace)
        .expect("panel should spawn");
    let open_attention = board.create_attention(
        source_workspace,
        Some(panel_id),
        "agent",
        "Needs input",
        AttentionSeverity::High,
    );
    let resolved_attention = board.create_attention(
        source_workspace,
        Some(panel_id),
        "agent-notify",
        "Task completed",
        AttentionSeverity::Medium,
    );
    assert!(board.resolve_attention(resolved_attention));

    board.assign_panel_to_workspace(panel_id, target_workspace);

    for attention_id in [open_attention, resolved_attention] {
        let attention = board
            .attention
            .iter()
            .find(|item| item.id == attention_id)
            .expect("panel attention should remain available");
        assert_eq!(attention.workspace_id, target_workspace);
    }
    assert!(
        board
            .attention
            .iter()
            .find(|item| item.id == resolved_attention)
            .is_some_and(AttentionItem::is_resolved)
    );
}

#[test]
fn remove_workspace_preserves_attention_for_reassigned_panel() {
    let mut board = Board::new();
    let source_workspace = board.create_workspace("source");
    let target_workspace = board.create_workspace("target");
    let panel_id = board
        .create_panel(editor_panel_options(), source_workspace)
        .expect("panel should spawn");
    let panel_attention = board.create_attention(
        source_workspace,
        Some(panel_id),
        "agent",
        "Needs input",
        AttentionSeverity::High,
    );
    let workspace_attention = board.create_attention(
        source_workspace,
        None,
        "system",
        "Workspace notice",
        AttentionSeverity::Low,
    );

    board.remove_workspace(source_workspace);

    assert!(board.workspace(source_workspace).is_none());
    assert_eq!(
        board.panel(panel_id).expect("reassigned panel").workspace_id,
        target_workspace
    );
    let attention = board
        .attention
        .iter()
        .find(|item| item.id == panel_attention)
        .expect("reassigned panel attention should survive workspace removal");
    assert_eq!(attention.workspace_id, target_workspace);
    assert!(attention.is_open());
    assert!(board.attention.iter().all(|item| item.id != workspace_attention));
}
