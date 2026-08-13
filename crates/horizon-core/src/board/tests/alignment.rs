use super::super::*;
use super::editor_panel_options;

// These exact f32 values leave a one-ULP horizontal residual after the first alignment.
const LARGE_FIRST_WORKSPACE_X: f32 = f32::from_bits(0x47ff_987b);
const LARGE_SECOND_WORKSPACE_X: f32 = f32::from_bits(0x47ff_ff93);
const LARGE_FIRST_PANEL_X: f32 = f32::from_bits(0x47ff_a27b);
const LARGE_FIRST_PANEL_WIDTH: f32 = f32::from_bits(0x430f_e65d);
const LARGE_SECOND_PANEL_X: f32 = f32::from_bits(0x4800_04ca);
const LARGE_SECOND_PANEL_WIDTH: f32 = f32::from_bits(0x43a2_9850);

fn geometry_bits(board: &Board) -> Vec<u32> {
    let mut bits = Vec::with_capacity(board.workspaces.len() * 2 + board.panels.len() * 4);
    for workspace in &board.workspaces {
        bits.extend(workspace.position.map(f32::to_bits));
    }
    for panel in &board.panels {
        bits.extend(panel.layout.position.map(f32::to_bits));
        bits.extend(panel.layout.size.map(f32::to_bits));
    }
    bits
}

fn assert_alignment_is_rejected_atomically(mut board: Board, workspace_ids: [WorkspaceId; 2]) {
    let before = geometry_bits(&board);
    assert!(board.align_workspaces_horizontally(&workspace_ids).is_none());
    assert_eq!(geometry_bits(&board), before);
}

fn large_coordinate_alignment_fixture() -> (Board, WorkspaceId, WorkspaceId) {
    let mut board = Board::new();
    let first_workspace = board.create_workspace_at("first", [LARGE_FIRST_WORKSPACE_X, 200.0]);
    let second_workspace = board.create_workspace_at("second", [LARGE_SECOND_WORKSPACE_X, 200.0]);
    let first_panel = board
        .create_panel(editor_panel_options(), first_workspace)
        .expect("first panel should spawn");
    let second_panel = board
        .create_panel(editor_panel_options(), second_workspace)
        .expect("second panel should spawn");
    let first_layout = &mut board.panel_mut(first_panel).expect("first panel").layout;
    first_layout.position[0] = LARGE_FIRST_PANEL_X;
    first_layout.size[0] = LARGE_FIRST_PANEL_WIDTH;
    let second_layout = &mut board.panel_mut(second_panel).expect("second panel").layout;
    second_layout.position[0] = LARGE_SECOND_PANEL_X;
    second_layout.size[0] = LARGE_SECOND_PANEL_WIDTH;

    (board, first_workspace, second_workspace)
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

    let alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace, third_workspace])
        .expect("aligned workspaces");

    assert_eq!(alignment.leftmost_workspace, first_workspace);
    assert!(alignment.positions_changed);

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
    let alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");

    assert_eq!(alignment.leftmost_workspace, first_workspace);
    assert!(alignment.positions_changed);
    let current_third_position = board.workspace(third_workspace).expect("third").position;
    assert!(
        vec2_eq(current_third_position, original_third_position),
        "expected detached workspace position {original_third_position:?}, got {current_third_position:?}"
    );
}

#[test]
fn align_workspaces_horizontally_is_idempotent_for_fractional_positions() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace_at("first", [2_000.0, 200.456]);
    let second_workspace = board.create_workspace_at("second", [4_076.76, 500.987]);

    let first_alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");
    assert!(first_alignment.positions_changed);
    let positions_after_first_alignment: Vec<_> = board.workspaces.iter().map(|workspace| workspace.position).collect();

    let second_alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");

    assert!(!second_alignment.positions_changed);
    assert_eq!(
        board
            .workspaces
            .iter()
            .map(|workspace| workspace.position)
            .collect::<Vec<_>>(),
        positions_after_first_alignment
    );
}

#[test]
fn align_workspaces_horizontally_is_idempotent_at_large_coordinates() {
    let (mut board, first_workspace, second_workspace) = large_coordinate_alignment_fixture();

    let first_alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");
    assert!(first_alignment.positions_changed);
    let geometry_after_first_alignment = geometry_bits(&board);

    let second_alignment = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");

    assert!(!second_alignment.positions_changed);
    assert_eq!(geometry_bits(&board), geometry_after_first_alignment);
}

#[test]
fn align_workspaces_horizontally_corrects_multi_ulp_misalignment_at_large_coordinates() {
    let (mut board, first_workspace, second_workspace) = large_coordinate_alignment_fixture();
    board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("initial alignment");
    assert!(board.translate_workspace(second_workspace, [0.0625, 0.0]));
    let misaligned_geometry = geometry_bits(&board);

    let correction = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("corrective alignment");

    assert!(correction.positions_changed);
    assert_ne!(geometry_bits(&board), misaligned_geometry);
}

#[test]
fn align_workspaces_horizontally_rejects_non_finite_geometry_without_mutation() {
    for invalid_coordinate in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for corrupt_panel in [false, true] {
            let mut board = Board::new();
            let first_workspace = board.create_workspace_at("first", [100.0, 200.0]);
            let invalid_workspace = board.create_workspace_at("invalid", [500.0, 300.0]);
            let third_workspace = board.create_workspace_at("third", [900.0, 400.0]);
            board
                .create_panel(editor_panel_options(), first_workspace)
                .expect("first panel should spawn");
            let invalid_panel = board
                .create_panel(editor_panel_options(), invalid_workspace)
                .expect("invalid panel should spawn");
            board
                .create_panel(editor_panel_options(), third_workspace)
                .expect("third panel should spawn");

            if corrupt_panel {
                board.panel_mut(invalid_panel).expect("invalid panel").layout.position[0] = invalid_coordinate;
            } else {
                board
                    .workspace_mut(invalid_workspace)
                    .expect("invalid workspace")
                    .position[0] = invalid_coordinate;
            }
            let before = geometry_bits(&board);

            let alignment = board.align_workspaces_horizontally(&[first_workspace, invalid_workspace, third_workspace]);

            assert!(alignment.is_none());
            assert_eq!(geometry_bits(&board), before);
        }
    }
}

#[test]
fn align_workspaces_horizontally_rejects_translation_overflow_without_mutation() {
    let mut workspace_overflow = Board::new();
    let first_workspace = workspace_overflow.create_workspace_at("first", [-2.0e38, 0.0]);
    let second_workspace = workspace_overflow.create_workspace_at("second", [2.0e38, 0.0]);
    let first_panel = workspace_overflow
        .create_panel(editor_panel_options(), first_workspace)
        .expect("first panel should spawn");
    let second_panel = workspace_overflow
        .create_panel(editor_panel_options(), second_workspace)
        .expect("second panel should spawn");
    let first_layout = &mut workspace_overflow.panel_mut(first_panel).expect("first panel").layout;
    first_layout.position[0] = -2.0e38;
    first_layout.size[0] = 3.0e38;
    workspace_overflow
        .panel_mut(second_panel)
        .expect("second panel")
        .layout
        .position[0] = -1.0e38;
    assert_alignment_is_rejected_atomically(workspace_overflow, [first_workspace, second_workspace]);

    let mut panel_overflow = Board::new();
    let first_workspace = panel_overflow.create_workspace_at("first", [0.0, 1.0e38]);
    let second_workspace = panel_overflow.create_workspace_at("second", [1_000.0, -1.0e38]);
    panel_overflow
        .create_panel(editor_panel_options(), first_workspace)
        .expect("first panel should spawn");
    let second_panel = panel_overflow
        .create_panel(editor_panel_options(), second_workspace)
        .expect("second panel should spawn");
    panel_overflow
        .panel_mut(second_panel)
        .expect("second panel")
        .layout
        .position[1] = 2.0e38;

    assert_alignment_is_rejected_atomically(panel_overflow, [first_workspace, second_workspace]);
}

#[test]
fn align_workspaces_horizontally_rejects_translated_frame_overflow_without_mutation() {
    let mut board = Board::new();
    let anchor_workspace = board.create_workspace_at("anchor", [0.0, 2.0e38]);
    let target_workspace = board.create_workspace_at("target", [1_000.0, -1.0e38]);
    board
        .create_panel(editor_panel_options(), anchor_workspace)
        .expect("anchor panel should spawn");
    let target_panel = board
        .create_panel(editor_panel_options(), target_workspace)
        .expect("target panel should spawn");
    let target_layout = &mut board.panel_mut(target_panel).expect("target panel").layout;
    target_layout.position[1] = -1.0e38;
    target_layout.size[1] = 2.0e38;

    assert_alignment_is_rejected_atomically(board, [anchor_workspace, target_workspace]);
}
