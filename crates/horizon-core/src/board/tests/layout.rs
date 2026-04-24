use crate::layout::{TILE_GAP, WS_INNER_PAD};
use crate::panel::DEFAULT_PANEL_SIZE;

use super::super::*;
use super::editor_panel_options;

#[test]
fn arranging_workspace_records_selected_layout() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn new_workspaces_use_grid_layout_by_default() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");

    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[0] + TILE_GAP,
            origin[1] + WS_INNER_PAD
        ]
    ));
    assert!(vec2_eq(
        board.panel(third).expect("third panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP
        ]
    ));
}

#[test]
fn default_grid_layout_accepts_fifth_panel() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    for index in 0..5 {
        board
            .create_panel(editor_panel_options(), workspace_id)
            .unwrap_or_else(|error| panic!("panel {} should spawn: {error}", index + 1));
    }

    let workspace = board.workspace(workspace_id).expect("workspace");
    assert_eq!(workspace.panels.len(), 5);
    assert_eq!(workspace.layout, Some(WorkspaceLayout::Grid));

    let fifth_panel = board.panel(workspace.panels[4]).expect("fifth panel");
    assert!(vec2_eq(
        fifth_panel.layout.position,
        [
            origin[0] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[0] + TILE_GAP,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
}

#[test]
fn adding_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");

    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
}

#[test]
fn closing_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    board.close_panel(first);

    assert!(vec2_eq(
        board.panel(second).expect("remaining panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn closing_middle_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    board.close_panel(second);

    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(third).expect("third panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn resizing_rows_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert!(board.resize_panel(first, [640.0, 420.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
    assert!(vec2_eq(first_panel.layout.size, [640.0, 420.0]));
    assert!(vec2_eq(second_panel.layout.size, [640.0, 420.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD + 420.0 + TILE_GAP]
    ));
}

#[test]
fn resizing_columns_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("columns");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Columns);

    assert!(board.resize_panel(first, [700.0, 360.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Columns)
    );
    assert!(vec2_eq(first_panel.layout.size, [700.0, 360.0]));
    assert!(vec2_eq(second_panel.layout.size, [700.0, 360.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD + 700.0 + TILE_GAP, origin[1] + WS_INNER_PAD]
    ));
}

#[test]
fn resizing_grid_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

    assert!(board.resize_panel(first, [610.0, 390.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    assert!(vec2_eq(first_panel.layout.size, [610.0, 390.0]));
    assert!(vec2_eq(second_panel.layout.size, [610.0, 390.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD + 610.0 + TILE_GAP, origin[1] + WS_INNER_PAD]
    ));
}

#[test]
fn adding_panel_preserves_live_rows_panel_size() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    assert!(board.resize_panel(first, [600.0, 400.0]));

    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");

    let second_panel = board.panel(second).expect("second panel");
    let third_panel = board.panel(third).expect("third panel");
    assert!(vec2_eq(second_panel.layout.size, [600.0, 400.0]));
    assert!(vec2_eq(third_panel.layout.size, [600.0, 400.0]));
    assert!(vec2_eq(
        third_panel.layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + 2.0 * (400.0 + TILE_GAP)
        ]
    ));
}

#[test]
fn resizing_rows_layout_pushes_neighbor_workspace_horizontally_when_width_growth_dominates() {
    let mut board = Board::new();
    let rows = board.create_workspace_at("rows", [0.0, 40.0]);
    let beta = board.create_workspace_at("beta", [630.0, 40.0]);

    let first = board
        .create_panel(editor_panel_options(), rows)
        .expect("first panel should spawn");
    board.arrange_workspace(rows, WorkspaceLayout::Rows);
    board
        .create_panel(editor_panel_options(), rows)
        .expect("second panel should spawn");
    board
        .create_panel(editor_panel_options(), beta)
        .expect("beta panel should spawn");

    let beta_before = board.workspace(beta).expect("beta workspace").position;

    assert!(board.resize_panel(first, [640.0, 420.0]));

    let beta_after = board.workspace(beta).expect("beta workspace").position;
    assert!(
        beta_after[0] > beta_before[0],
        "expected beta to move right from {beta_before:?}, got {beta_after:?}"
    );
    assert!(
        (beta_after[1] - beta_before[1]).abs() <= f32::EPSILON,
        "expected beta y to stay at {}, got {}",
        beta_before[1],
        beta_after[1],
    );
}

#[test]
fn manual_panel_move_returns_workspace_to_freeform() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert!(board.move_panel(panel_id, [420.0, 360.0]));

    assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
}

#[test]
fn clearing_workspace_layout_preserves_current_panel_positions() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let arranged_position = board.panel(panel_id).expect("panel").layout.position;

    assert!(board.clear_workspace_layout(workspace_id));

    assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
    let current_position = board.panel(panel_id).expect("panel").layout.position;
    assert!(
        vec2_eq(current_position, arranged_position),
        "expected {arranged_position:?}, got {current_position:?}"
    );
}
