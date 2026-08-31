use crate::layout::TILE_GAP;
use crate::{CanvasViewState, RuntimeState, WindowConfig};

use super::super::*;
use super::editor_panel_options;

fn arranged_board(layout: WorkspaceLayout, count: usize) -> (Board, WorkspaceId, Vec<PanelId>) {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("arranged");
    let panel_ids = (0..count)
        .map(|index| {
            board
                .create_panel(editor_panel_options(), workspace_id)
                .unwrap_or_else(|error| panic!("panel {} should spawn: {error}", index + 1))
        })
        .collect();
    board.arrange_workspace(workspace_id, layout);
    (board, workspace_id, panel_ids)
}

#[test]
fn collision_target_uses_the_dragged_panel_center_for_every_layout() {
    for layout in WorkspaceLayout::ALL {
        let (board, _, panel_ids) = arranged_board(layout, 4);
        let source = panel_ids[0];
        let target = panel_ids[3];
        let target_position = board.panel(target).expect("target panel").layout.position;

        assert_eq!(
            board.arranged_panel_collision_target(source, target_position),
            Some(target),
            "{layout:?} should detect the slot under the dragged panel center"
        );
        assert_eq!(
            board.arranged_panel_collision_target(source, [100_000.0, 100_000.0]),
            None,
            "{layout:?} should ignore empty canvas"
        );
    }
}

#[test]
fn collision_target_ignores_the_gap_and_far_edge() {
    let (board, _, panel_ids) = arranged_board(WorkspaceLayout::Columns, 2);
    let source = board.panel(panel_ids[0]).expect("source panel");
    let target = board.panel(panel_ids[1]).expect("target panel");
    let source_size = source.layout.size;
    let target_position = target.layout.position;

    let gap_center_x = target_position[0] - TILE_GAP * 0.5;
    let gap_position = [gap_center_x - source_size[0] * 0.5, target_position[1]];
    assert_eq!(board.arranged_panel_collision_target(source.id, gap_position), None);

    let far_edge_position = [
        target_position[0] + target.layout.size[0] - source_size[0] * 0.5,
        target_position[1],
    ];
    assert_eq!(
        board.arranged_panel_collision_target(source.id, far_edge_position),
        None
    );
}

#[test]
fn swapping_arranged_panels_preserves_layout_focus_size_and_slots() {
    for layout in WorkspaceLayout::ALL {
        let (mut board, workspace_id, panel_ids) = arranged_board(layout, 4);
        let source = panel_ids[0];
        let target = panel_ids[3];
        assert!(board.resize_panel(source, [610.0, 390.0]));
        board.focus(source);

        let source_slot = board.panel(source).expect("source panel").layout.position;
        let target_slot = board.panel(target).expect("target panel").layout.position;

        assert!(board.swap_arranged_panels(source, target));

        let workspace = board.workspace(workspace_id).expect("workspace");
        assert_eq!(workspace.layout, Some(layout));
        assert_eq!(workspace.panels, [target, panel_ids[1], panel_ids[2], source]);
        assert_eq!(board.focused, Some(source));
        assert!(vec2_eq(
            board.panel(source).expect("source panel").layout.position,
            target_slot
        ));
        assert!(vec2_eq(
            board.panel(target).expect("target panel").layout.position,
            source_slot
        ));
        for panel_id in panel_ids {
            assert!(vec2_eq(
                board.panel(panel_id).expect("panel").layout.size,
                [610.0, 390.0]
            ));
        }
    }
}

#[test]
fn only_visible_siblings_in_an_arranged_workspace_can_swap() {
    let (mut board, workspace_id, panel_ids) = arranged_board(WorkspaceLayout::Grid, 3);
    let source = panel_ids[0];
    let hidden = panel_ids[1];
    let original_order = board.workspace(workspace_id).expect("workspace").panels.clone();
    assert!(board.set_panel_visible(hidden, false));

    assert_ne!(
        board.arranged_panel_collision_target(source, board.panel(hidden).expect("hidden panel").layout.position),
        Some(hidden)
    );
    assert!(!board.swap_arranged_panels(source, hidden));

    let other_workspace = board.create_workspace("other");
    let other = board
        .create_panel(editor_panel_options(), other_workspace)
        .expect("other panel should spawn");
    assert!(!board.swap_arranged_panels(source, other));

    assert!(board.clear_workspace_layout(workspace_id));
    assert!(!board.swap_arranged_panels(source, panel_ids[2]));
    assert_eq!(board.workspace(workspace_id).expect("workspace").panels, original_order);
}

#[test]
fn sequential_grid_collisions_move_the_dragged_panel_through_slots() {
    let (mut board, workspace_id, panel_ids) = arranged_board(WorkspaceLayout::Grid, 4);
    let source = panel_ids[0];

    for target in [panel_ids[1], panel_ids[3]] {
        let target_position = board.panel(target).expect("target panel").layout.position;
        assert_eq!(
            board.arranged_panel_collision_target(source, target_position),
            Some(target)
        );
        assert!(board.swap_arranged_panels(source, target));
    }

    assert_eq!(
        board.workspace(workspace_id).expect("workspace").panels,
        [panel_ids[1], panel_ids[3], panel_ids[2], source]
    );
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
}

#[test]
fn swapped_slot_order_survives_runtime_state_round_trip() {
    let (mut board, _, panel_ids) = arranged_board(WorkspaceLayout::Rows, 3);
    assert!(board.swap_arranged_panels(panel_ids[0], panel_ids[2]));

    let expected_local_ids = board.workspaces[0]
        .panels
        .iter()
        .map(|panel_id| board.panel(*panel_id).expect("panel").local_id.clone())
        .collect::<Vec<_>>();
    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let saved_local_ids = state.workspaces[0]
        .panels
        .iter()
        .map(|panel| panel.local_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(saved_local_ids, expected_local_ids);

    let restored = Board::from_runtime_state(&state).expect("runtime state should restore");
    let restored_local_ids = restored.workspaces[0]
        .panels
        .iter()
        .map(|panel_id| restored.panel(*panel_id).expect("panel").local_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(restored_local_ids, expected_local_ids);
    assert_eq!(restored.workspaces[0].layout, Some(WorkspaceLayout::Rows));
}
