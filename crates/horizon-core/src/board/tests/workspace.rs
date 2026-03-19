use std::path::PathBuf;

use crate::config::WorkspaceConfig;
use crate::layout::{TILE_GAP, WS_INNER_PAD};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelOptions};
use crate::runtime_state::WorkspaceTemplateRef;

use super::super::*;
use super::editor_panel_options;

#[test]
fn panels_tile_within_workspace_region() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let workspace_position = board.workspace(workspace_id).expect("workspace").position;

    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    let panel = board.panel(panel_id).expect("panel should exist");

    assert!((panel.layout.position[0] - (workspace_position[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[1] - (workspace_position[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
}

#[test]
fn workspaces_are_placed_apart() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    board
        .create_panel(editor_panel_options(), first_workspace)
        .expect("panel should spawn");
    let second_workspace = board.create_workspace("second");

    let first_position = board.workspace(first_workspace).expect("first workspace").position;
    let second_position = board.workspace(second_workspace).expect("second workspace").position;

    assert!(second_position[0] > first_position[0] + DEFAULT_PANEL_SIZE[0]);
}

#[test]
fn assign_panel_moves_it_to_target_workspace() {
    let mut board = Board::new();
    let source_workspace = board.create_workspace("source");
    let target_workspace = board.create_workspace("target");

    let panel_id = board
        .create_panel(editor_panel_options(), source_workspace)
        .expect("panel should spawn");

    let target_position = board.workspace(target_workspace).expect("target workspace").position;
    board.assign_panel_to_workspace(panel_id, target_workspace);

    let panel = board.panel(panel_id).expect("panel");
    assert_eq!(panel.workspace_id, target_workspace);
    assert!(panel.layout.position[0] >= target_position[0]);
}

#[test]
fn translating_workspace_moves_workspace_origin_and_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    let original_workspace_pos = board.workspace(workspace_id).expect("workspace").position;
    let original_panel_pos = board.panel(panel_id).expect("panel").layout.position;

    assert!(board.translate_workspace(workspace_id, [48.0, 24.0]));

    let workspace = board.workspace(workspace_id).expect("workspace");
    let panel = board.panel(panel_id).expect("panel");
    assert!((workspace.position[0] - (original_workspace_pos[0] + 48.0)).abs() <= f32::EPSILON);
    assert!((workspace.position[1] - (original_workspace_pos[1] + 24.0)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[0] - (original_panel_pos[0] + 48.0)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[1] - (original_panel_pos[1] + 24.0)).abs() <= f32::EPSILON);
}

#[test]
fn sync_workspace_metadata_updates_only_templated_workspaces() {
    let mut board = Board::new();
    let templated_workspace = board.create_workspace("stale");
    let manual_workspace = board.create_workspace("manual");

    {
        let workspace = board.workspace_mut(templated_workspace).expect("templated workspace");
        workspace.template = Some(WorkspaceTemplateRef {
            workspace_index: 0,
            workspace_name: "template".to_string(),
        });
        workspace.cwd = Some(PathBuf::from("/tmp/old"));
    }
    {
        let workspace = board.workspace_mut(manual_workspace).expect("manual workspace");
        workspace.cwd = Some(PathBuf::from("/tmp/manual"));
    }

    let config = Config {
        workspaces: vec![WorkspaceConfig {
            name: "synced".to_string(),
            color: None,
            cwd: Some("~/repo".to_string()),
            position: None,
            terminals: Vec::new(),
        }],
        ..Config::default()
    };

    board.sync_workspace_metadata(&config);

    let templated = board.workspace(templated_workspace).expect("templated workspace");
    assert_eq!(templated.name, "synced");
    assert_eq!(templated.cwd, Some(Config::expand_tilde("~/repo")));

    let manual = board.workspace(manual_workspace).expect("manual workspace");
    assert_eq!(manual.name, "manual");
    assert_eq!(manual.cwd, Some(PathBuf::from("/tmp/manual")));
}

#[test]
fn explicit_position_overrides_default_tiling() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("test");

    board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");

    let click_pos = [800.0, 600.0];
    let panel_id = board
        .create_panel(
            PanelOptions {
                position: Some(click_pos),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("second panel should spawn");

    let panel = board.panel(panel_id).expect("panel should exist");
    assert!(
        (panel.layout.position[0] - click_pos[0]).abs() <= f32::EPSILON
            && (panel.layout.position[1] - click_pos[1]).abs() <= f32::EPSILON,
        "panel should be placed at the explicit click position ({click_pos:?}), not at the default tile position ({:?})",
        panel.layout.position,
    );
}

#[test]
fn close_panels_in_workspace_keeps_workspace_available() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("alpha");
    let other_workspace_id = board.create_workspace("beta");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let other_panel = board
        .create_panel(editor_panel_options(), other_workspace_id)
        .expect("other panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let closed = board.close_panels_in_workspace(workspace_id);
    board.remove_empty_workspaces();

    assert_eq!(closed, vec![first, second]);
    assert!(board.panel(first).is_none());
    assert!(board.panel(second).is_none());
    assert!(board.panel(other_panel).is_some());
    assert_eq!(board.focused, None);
    assert_eq!(board.active_workspace, Some(workspace_id));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
    assert!(board.workspace(workspace_id).expect("workspace").panels.is_empty());
}

#[test]
fn align_workspaces_horizontally_arranges_in_row() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    let second_workspace = board.create_workspace("second");
    let third_workspace = board.create_workspace("third");

    board.move_workspace(first_workspace, [100.0, 200.0]);
    board.move_workspace(second_workspace, [500.0, 50.0]);
    board.move_workspace(third_workspace, [300.0, 400.0]);

    board.align_workspaces_horizontally(&[first_workspace, second_workspace, third_workspace]);

    let first_position = board.workspace(first_workspace).expect("first").position;
    let third_position = board.workspace(third_workspace).expect("third").position;
    let second_position = board.workspace(second_workspace).expect("second").position;

    assert!((first_position[1] - third_position[1]).abs() <= f32::EPSILON);
    assert!((third_position[1] - second_position[1]).abs() <= f32::EPSILON);
    assert!(third_position[0] > first_position[0], "third should be right of first");
    assert!(
        second_position[0] > third_position[0],
        "second should be right of third"
    );
}

#[test]
fn align_workspaces_horizontally_only_moves_selected_workspaces() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    let second_workspace = board.create_workspace("second");
    let third_workspace = board.create_workspace("third");

    board.move_workspace(first_workspace, [100.0, 200.0]);
    board.move_workspace(second_workspace, [500.0, 50.0]);
    board.move_workspace(third_workspace, [20.0, 20.0]);

    let original_third_position = board.workspace(third_workspace).expect("third").position;
    let leftmost = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");

    assert_eq!(leftmost, first_workspace);
    let current_third_position = board.workspace(third_workspace).expect("third").position;
    assert!(
        vec2_eq(current_third_position, original_third_position),
        "expected detached workspace position {original_third_position:?}, got {current_third_position:?}"
    );
}

#[test]
fn resize_panel_pushes_sibling() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("test");

    let panel_a = board
        .create_panel(
            PanelOptions {
                position: Some([0.0, 0.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel A should spawn");
    let panel_b = board
        .create_panel(
            PanelOptions {
                position: Some([220.0, 0.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel B should spawn");

    board.resize_panel(panel_a, [300.0, 200.0]);

    let panel_b_position = board.panel(panel_b).expect("panel B").layout.position;
    assert!(
        panel_b_position[0] >= 300.0 + TILE_GAP - 1.0,
        "panel B should be pushed right, got x={}",
        panel_b_position[0],
    );
}
