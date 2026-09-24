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
    assert!(board.release_empty_workspace_retention(cloud), "the hold is released");
    assert!(board.workspace(cloud).is_some(), "removal waits for the cleanup pass");
    board.remove_empty_workspaces();

    assert!(board.workspace(cloud).is_none());
    assert_eq!(board.active_workspace, Some(local));
    assert_eq!(board.focused, Some(remaining));
}

#[derive(Clone, Copy, Debug)]
enum Departure {
    /// The focused last panel of a plain workspace closes.
    LastPanel,
    /// A cloud whose focused member is the workspace's last panel is removed.
    CloudWithMember,
    /// An empty cloud in the active workspace, with nothing focused, is removed.
    EmptyCloud,
}

#[test]
fn removing_a_cloud_moves_focus_like_closing_the_last_panel() {
    let outcome = |departure: Departure| {
        let mut board = Board::new();
        let first = board.create_workspace("first");
        board
            .create_panel(editor_panel_options(), first)
            .expect("panel should spawn");
        let second = board.create_workspace("second");
        let most_recent = board
            .create_panel(editor_panel_options(), second)
            .expect("panel should spawn");
        let target = board.create_workspace("target");
        if !matches!(departure, Departure::LastPanel) {
            add_cloud(&mut board, 1, target);
        }
        if matches!(departure, Departure::EmptyCloud) {
            board.focus_workspace(target);
            assert_eq!(board.focused, None);
        } else {
            let last = board
                .create_panel(editor_panel_options(), target)
                .expect("panel should spawn");
            assert_eq!(board.focused, Some(last));
            board.close_panel(last);
        }
        if !matches!(departure, Departure::LastPanel) {
            remove_cloud(&mut board, 1);
            board.release_empty_workspace_retention(target);
        }
        board.remove_empty_workspaces();
        assert!(board.workspace(target).is_none(), "{departure:?}");
        assert_eq!(board.focused, Some(most_recent), "{departure:?}");
        assert_eq!(board.active_workspace, Some(second), "{departure:?}");
        (board.focused, board.active_workspace, board.workspaces.len())
    };

    let closed = outcome(Departure::LastPanel);
    assert_eq!(outcome(Departure::CloudWithMember), closed);
    assert_eq!(outcome(Departure::EmptyCloud), closed);
}

#[test]
fn releasing_the_active_workspace_of_an_empty_board_selects_the_first_workspace() {
    let mut board = Board::new();
    let first = board.create_workspace("first");
    let cloud = board.create_workspace("cloud");
    add_cloud(&mut board, 1, cloud);
    board.focus_workspace(cloud);
    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(cloud);
    board.remove_empty_workspaces();

    assert!(board.workspace(cloud).is_none());
    assert_eq!(board.focused, None);
    assert_eq!(board.active_workspace, Some(first));
}

#[test]
fn workspace_stays_while_another_cloud_or_a_panel_uses_it() {
    let mut board = Board::new();
    let shared = board.create_workspace("shared");
    add_cloud(&mut board, 1, shared);
    add_cloud(&mut board, 2, shared);
    remove_cloud(&mut board, 1);
    assert!(
        !board.release_empty_workspace_retention(shared),
        "cloud 2 still holds it"
    );
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
