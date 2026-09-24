use std::path::PathBuf;

use crate::cloud_panel::CloudGroup;

use super::super::*;
use super::editor_panel_options;

/// Record a cloud in `workspace` and keep the workspace like the per-frame UI pass does.
fn add_cloud(board: &mut Board, issue: u32, workspace: WorkspaceId) {
    let local_id = board.workspace(workspace).expect("workspace").local_id.clone();
    board.cloud_groups.0.push(CloudGroup::new(
        issue,
        format!("Cloud {issue}"),
        local_id,
        PathBuf::new(),
        [0.0, 0.0],
    ));
    board.retain_workspace_when_empty(workspace);
}

fn remove_cloud(board: &mut Board, issue: u32) {
    board.cloud_groups.0.retain(|group| group.issue != issue);
}

#[test]
fn released_cloud_workspace_is_removed_by_the_next_cleanup() {
    let mut board = Board::new();
    let local = board.create_workspace("local");
    let remaining = board
        .create_panel(editor_panel_options(), local)
        .expect("panel should spawn");
    let cloud = board.create_workspace("cloud");
    add_cloud(&mut board, 1, cloud);
    board.active_workspace = Some(cloud);
    board.focused = None;
    board.remove_empty_workspaces();
    assert!(board.workspace(cloud).is_some(), "an empty cloud keeps its workspace");

    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(cloud);
    assert!(board.workspace(cloud).is_some(), "removal waits for the cleanup pass");
    board.remove_empty_workspaces();

    assert!(board.workspace(cloud).is_none());
    assert_eq!(board.active_workspace, Some(local));
    assert!(board.panel(remaining).is_some());
}

#[test]
fn removing_a_cloud_moves_focus_like_closing_the_last_panel() {
    let outcome = |with_cloud: bool| {
        let mut board = Board::new();
        let local = board.create_workspace("local");
        let remaining = board
            .create_panel(editor_panel_options(), local)
            .expect("panel should spawn");
        let target = board.create_workspace("target");
        let last = board
            .create_panel(editor_panel_options(), target)
            .expect("panel should spawn");
        if with_cloud {
            add_cloud(&mut board, 1, target);
        }
        assert_eq!(board.focused, Some(last));
        board.close_panel(last);
        if with_cloud {
            remove_cloud(&mut board, 1);
            board.release_empty_workspace_retention(target);
        }
        board.remove_empty_workspaces();
        assert!(board.workspace(target).is_none());
        assert_eq!(board.focused, Some(remaining));
        (board.focused, board.active_workspace, board.workspaces.len())
    };

    assert_eq!(outcome(true), outcome(false));
}

#[test]
fn workspace_stays_while_another_cloud_or_a_panel_uses_it() {
    let mut board = Board::new();
    let shared = board.create_workspace("shared");
    add_cloud(&mut board, 1, shared);
    add_cloud(&mut board, 2, shared);
    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(shared);
    board.remove_empty_workspaces();
    assert!(board.workspace(shared).is_some(), "the other cloud keeps its workspace");

    let mixed = board.create_workspace("mixed");
    add_cloud(&mut board, 3, mixed);
    let hidden = board
        .create_panel(editor_panel_options(), mixed)
        .expect("panel should spawn");
    board.retain_workspace_when_empty(mixed);
    assert!(board.set_panel_visible(hidden, false));
    remove_cloud(&mut board, 3);
    board.release_empty_workspace_retention(mixed);
    board.remove_empty_workspaces();
    assert!(board.workspace(mixed).is_some(), "a hidden panel keeps its workspace");

    board.close_panel(hidden);
    assert!(
        board.workspace(mixed).is_none(),
        "without a cloud it closes with its last panel like any workspace"
    );
}

#[test]
fn close_all_panels_keeps_a_cloud_workspace_until_its_cloud_is_removed() {
    let mut board = Board::new();
    let workspace = board.create_workspace("cloud");
    add_cloud(&mut board, 1, workspace);
    board
        .create_panel(editor_panel_options(), workspace)
        .expect("panel should spawn");
    board.retain_workspace_when_empty(workspace);

    board.close_panels_in_workspace(workspace);
    board.remove_empty_workspaces();
    assert!(board.workspace(workspace).is_some());

    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(workspace);
    board.remove_empty_workspaces();
    assert!(board.workspace(workspace).is_none());
}

#[test]
fn legacy_remote_view_stays_after_release() {
    let mut board = Board::new();
    let remote = board.create_workspace("Legacy remote");
    board.workspace_mut(remote).expect("workspace").remote_workspace = Some(
        crate::RemoteWorkspaceReference::new("123e4567-e89b-42d3-a456-426614174000".into(), "remote-workspace".into())
            .expect("reference"),
    );
    board.release_empty_workspace_retention(remote);
    board.remove_empty_workspaces();
    assert!(board.workspace(remote).is_some());
}
