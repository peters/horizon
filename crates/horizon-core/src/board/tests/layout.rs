use crate::layout::{TILE_GAP, WS_INNER_PAD};
use crate::panel::DEFAULT_PANEL_SIZE;

use super::super::*;
use super::editor_panel_options;

#[test]
#[cfg(feature = "cloud-workspaces")]
fn parent_presets_and_reflow_preserve_cloud_layouts() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};
    use std::path::PathBuf;
    for layout in [None, Some(WorkspaceLayout::Rows), Some(WorkspaceLayout::Grid)] {
        let mut board = Board::new();
        let workspace = board.create_workspace("cloud");
        let first = board.create_panel(editor_panel_options(), workspace).unwrap();
        let second = board.create_panel(editor_panel_options(), workspace).unwrap();
        let local = board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(1, "Cloud".into(), local, PathBuf::default(), [24.0, 128.0]);
        group.attach(&mut board, first);
        group.attach(&mut board, second);
        group.set_layout(&mut board, layout);
        board.cloud_groups = CloudGroups(vec![group]);
        let geometry = |board: &Board| {
            [first, second].map(|id| {
                let panel = board.panel(id).unwrap();
                (panel.layout.position, panel.layout.size)
            })
        };
        let before = geometry(&board);
        for parent in WorkspaceLayout::ALL {
            board.arrange_workspace(workspace, parent);
            assert_eq!(geometry(&board), before);
            assert_eq!(board.workspace(workspace).unwrap().layout, Some(parent));
        }
        board.workspace_mut(workspace).unwrap().layout = Some(WorkspaceLayout::Columns);
        let added = board.create_panel(editor_panel_options(), workspace).unwrap();
        assert_eq!(geometry(&board), before);
        let mut groups = board.cloud_groups.clone();
        groups.reconcile(&mut board);
        assert_eq!(geometry(&board), before);
        assert_eq!(groups.0[0].layout, layout);
        assert_eq!(
            board.workspace(workspace).unwrap().layout,
            Some(WorkspaceLayout::Columns)
        );
        // Keep this resize about reflow, clear of the cloud.
        board.move_panel(added, [0.0, 4000.0]);
        let size = board.panel(added).unwrap().layout.size;
        board.workspace_mut(workspace).unwrap().layout = Some(WorkspaceLayout::Grid);
        board.resize_panel(added, [size[0] + 25.0, size[1] + 25.0]);
        assert_eq!(geometry(&board), before);
        assert!(vec2_eq(
            board.panel(added).unwrap().layout.size,
            [size[0] + 25.0, size[1] + 25.0]
        ));
    }
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn workspace_layout_arranges_free_panels_without_moving_cloud_members() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};

    let mut board = Board::new();
    let workspace = board.create_workspace("mixed");
    let origin = board.workspace(workspace).unwrap().position;
    let free_first = board.create_panel(editor_panel_options(), workspace).unwrap();
    let free_second = board.create_panel(editor_panel_options(), workspace).unwrap();
    let cloud_first = board.create_panel(editor_panel_options(), workspace).unwrap();
    let cloud_second = board.create_panel(editor_panel_options(), workspace).unwrap();
    let local = board.workspace(workspace).unwrap().local_id.clone();
    let mut group = CloudGroup::new(1, "Cloud".into(), local, std::path::PathBuf::new(), [24.0, 900.0]);
    group.attach(&mut board, cloud_first);
    group.attach(&mut board, cloud_second);
    group.set_layout(&mut board, Some(WorkspaceLayout::Rows));
    let cloud_layout = group.layout;
    board.cloud_groups = CloudGroups(vec![group]);
    let cloud_geometry = [cloud_first, cloud_second].map(|id| {
        let panel = board.panel(id).unwrap();
        (panel.layout.position, panel.layout.size)
    });

    board.arrange_workspace(workspace, WorkspaceLayout::Columns);

    assert_eq!(
        board.workspace(workspace).unwrap().layout,
        Some(WorkspaceLayout::Columns)
    );
    assert_eq!(board.cloud_groups.0[0].layout, cloud_layout);
    assert_eq!(
        [cloud_first, cloud_second].map(|id| {
            let panel = board.panel(id).unwrap();
            (panel.layout.position, panel.layout.size)
        }),
        cloud_geometry
    );
    let free_first_panel = board.panel(free_first).unwrap();
    let free_second_panel = board.panel(free_second).unwrap();
    assert!(vec2_eq(
        free_first_panel.layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(free_first_panel.layout.size, free_second_panel.layout.size));
    assert!(free_second_panel.layout.position[0] > free_first_panel.layout.position[0]);
    assert!((free_second_panel.layout.position[1] - free_first_panel.layout.position[1]).abs() <= f32::EPSILON);

    let mut groups = board.cloud_groups.clone();
    groups.reconcile(&mut board);
    assert_eq!(
        board.workspace(workspace).unwrap().layout,
        Some(WorkspaceLayout::Columns)
    );
    assert_eq!(
        [cloud_first, cloud_second].map(|id| {
            let panel = board.panel(id).unwrap();
            (panel.layout.position, panel.layout.size)
        }),
        cloud_geometry
    );

    let member_position = board.panel(cloud_first).unwrap().layout.position;
    assert!(board.move_panel(cloud_first, [member_position[0] + 12.0, member_position[1] + 8.0]));
    assert_eq!(
        board.workspace(workspace).unwrap().layout,
        Some(WorkspaceLayout::Columns)
    );
    assert!(board.move_panel(free_first, [480.0, 360.0]));
    assert_eq!(board.workspace(workspace).unwrap().layout, None);
    assert_eq!(board.cloud_groups.0[0].layout, Some(WorkspaceLayout::Rows));
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn workspace_grid_does_not_cover_an_attached_cloud() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};

    let mut board = Board::new();
    let workspace = board.create_workspace("mixed");
    let origin = board.workspace(workspace).unwrap().position;
    let _left = board.create_panel(editor_panel_options(), workspace).unwrap();
    let _right = board.create_panel(editor_panel_options(), workspace).unwrap();
    let local = board.workspace(workspace).unwrap().local_id.clone();
    // Sit the cloud on the workspace origin, where Grid would otherwise place a panel.
    let cloud_position = [origin[0] + 8.0, origin[1] + 8.0];
    let group = CloudGroup::new(1, "Cloud".into(), local, std::path::PathBuf::new(), cloud_position);
    let (cloud_min, cloud_max) = group.overview_bounds();
    let cloud_rect = [cloud_min[0], cloud_min[1], cloud_max[0], cloud_max[1]];
    board.cloud_groups = CloudGroups(vec![group]);

    board.arrange_workspace(workspace, WorkspaceLayout::Grid);

    assert!(vec2_eq(board.cloud_groups.0[0].position, cloud_position));
    assert_eq!(board.workspace(workspace).unwrap().layout, Some(WorkspaceLayout::Grid));
    for panel in board.panels.iter().filter(|panel| panel.workspace_id == workspace) {
        let rect = [
            panel.layout.position[0],
            panel.layout.position[1],
            panel.layout.position[0] + panel.layout.size[0],
            panel.layout.position[1] + panel.layout.size[1],
        ];
        assert!(
            rect[2] <= cloud_rect[0]
                || cloud_rect[2] <= rect[0]
                || rect[3] <= cloud_rect[1]
                || cloud_rect[3] <= rect[1],
            "grid panel {:?} covers the cloud {cloud_rect:?}",
            panel.layout.position
        );
    }
    let (frame_min, frame_max) = board.workspace_bounds(workspace).expect("workspace frame");
    assert!(frame_min[0] <= cloud_rect[0] && frame_min[1] <= cloud_rect[1]);
    assert!(frame_max[0] >= cloud_rect[2] && frame_max[1] >= cloud_rect[3]);
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn workspace_grid_clears_a_row_of_clouds_without_a_step_limit() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};
    use crate::layout::TILE_GAP;

    let mut board = Board::new();
    let workspace = board.create_workspace("row");
    let origin = board.workspace(workspace).unwrap().position;
    let panel_id = board.create_panel(editor_panel_options(), workspace).unwrap();
    let local = board.workspace(workspace).unwrap().local_id.clone();
    let sample = CloudGroup::new(1, "Cloud".into(), local.clone(), std::path::PathBuf::new(), origin);
    let (sample_min, sample_max) = sample.overview_bounds();
    let pitch = (sample_max[0] - sample_min[0]) + TILE_GAP;
    let mut cursor_x = origin[0];
    let groups = (0..9)
        .map(|index| {
            let position = [cursor_x, origin[1]];
            cursor_x += pitch;
            CloudGroup::new(
                index + 1,
                format!("Cloud {index}"),
                local.clone(),
                std::path::PathBuf::new(),
                position,
            )
        })
        .collect();
    board.cloud_groups = CloudGroups(groups);
    let obstacles: Vec<[f32; 4]> = board
        .cloud_groups
        .0
        .iter()
        .map(|group| {
            let (min, max) = group.overview_bounds();
            [min[0], min[1], max[0], max[1]]
        })
        .collect();

    board.arrange_workspace(workspace, WorkspaceLayout::Grid);

    let panel = board.panel(panel_id).unwrap();
    let rect = [
        panel.layout.position[0],
        panel.layout.position[1],
        panel.layout.position[0] + panel.layout.size[0] + 2.0 * super::super::PANEL_CHROME_PAD,
        panel.layout.position[1]
            + panel.layout.size[1]
            + super::super::PANEL_CHROME_TITLEBAR
            + 2.0 * super::super::PANEL_CHROME_PAD,
    ];
    for obstacle in &obstacles {
        assert!(
            rect[2] <= obstacle[0] || obstacle[2] <= rect[0] || rect[3] <= obstacle[1] || obstacle[3] <= rect[1],
            "panel {rect:?} still covers cloud {obstacle:?}"
        );
    }
    assert!(vec2_eq(board.cloud_groups.0[0].position, origin));
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn translating_a_workspace_moves_its_cloud_with_the_frame() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};

    let mut board = Board::new();
    let workspace = board.create_workspace("mixed");
    let _panel = board.create_panel(editor_panel_options(), workspace).unwrap();
    let local = board.workspace(workspace).unwrap().local_id.clone();
    let group = CloudGroup::new(
        1,
        "Cloud".into(),
        local,
        std::path::PathBuf::new(),
        board.workspace(workspace).unwrap().position,
    );
    board.cloud_groups = CloudGroups(vec![group]);
    let mut groups = board.cloud_groups.clone();
    groups.reconcile(&mut board);
    let synced = board.cloud_groups.0[0].position;
    let before = board.workspace_frame_rect(workspace).expect("frame");

    assert!(board.translate_workspace_with_push(workspace, [80.0, 0.0]));

    assert!(vec2_eq(board.cloud_groups.0[0].position, [synced[0] + 80.0, synced[1]]));
    groups = board.cloud_groups.clone();
    groups.reconcile(&mut board);
    assert!(vec2_eq(board.cloud_groups.0[0].position, [synced[0] + 80.0, synced[1]]));
    let after = board.workspace_frame_rect(workspace).expect("translated frame");
    assert!((after[0] - before[0] - 80.0).abs() <= 0.5);
    assert!((after[2] - before[2] - 80.0).abs() <= 0.5);
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn removing_a_cloud_returns_arranged_panels_to_the_preset() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};

    let mut board = Board::new();
    let workspace = board.create_workspace("mixed");
    let panel = board.create_panel(editor_panel_options(), workspace).unwrap();
    let local = board.workspace(workspace).unwrap().local_id.clone();
    let origin = board.workspace(workspace).unwrap().position;
    let group = CloudGroup::new(1, "Cloud".into(), local, std::path::PathBuf::new(), origin);
    board.cloud_groups = CloudGroups(vec![group]);
    board.arrange_workspace(workspace, WorkspaceLayout::Grid);
    let grid_size = board.panel(panel).unwrap().layout.size;
    let shifted = board.panel(panel).unwrap().layout.position;
    assert!(!vec2_eq(shifted, [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]));
    board.arrange_workspace(workspace, WorkspaceLayout::Rows);
    assert!(vec2_eq(board.panel(panel).unwrap().layout.size, grid_size));
    board.arrange_workspace(workspace, WorkspaceLayout::Grid);

    let grid_size = board.panel(panel).unwrap().layout.size;
    board.arrange_workspace(workspace, WorkspaceLayout::Rows);
    let rows_size = board.panel(panel).unwrap().layout.size;
    assert!(rows_size[0] <= grid_size[0] + 1.0);
    assert!(rows_size[1] <= grid_size[1] + 1.0);

    board.cloud_groups.0.clear();
    board.reapply_workspace_layout_if_set(workspace);

    assert!(vec2_eq(
        board.panel(panel).unwrap().layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
}

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

#[test]
fn switching_from_manual_to_preset_arranges_immediately() {
    for layout in WorkspaceLayout::ALL {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("manual");

        let first = board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("second panel should spawn");

        // Placing a panel manually returns the workspace to freeform. The
        // second panel sits far enough out that the fitted sizes below stay
        // above the default-size clamp on every axis.
        assert!(board.move_panel(first, [180.0, 140.0]));
        assert!(board.move_panel(second, [700.0, 900.0]));
        assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);

        let origin = board.workspace(workspace_id).expect("workspace").position;
        let content_min = [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD];
        let mut content = [0.0f32; 2];
        for id in [first, second] {
            let panel = board.panel(id).expect("panel");
            content[0] = content[0].max(panel.layout.position[0] + panel.layout.size[0] - content_min[0]);
            content[1] = content[1].max(panel.layout.position[1] + panel.layout.size[1] - content_min[1]);
        }
        // Two panels: Rows stacks them, Columns and Grid sit them side by side.
        let expected_size = match layout {
            WorkspaceLayout::Rows => [content[0], (content[1] - TILE_GAP) / 2.0],
            WorkspaceLayout::Columns | WorkspaceLayout::Grid => [(content[0] - TILE_GAP) / 2.0, content[1]],
        };
        assert!(
            expected_size[0] > DEFAULT_PANEL_SIZE[0] && expected_size[1] > DEFAULT_PANEL_SIZE[1],
            "test setup must keep fitted sizes above the default-size clamp, got {expected_size:?}"
        );

        board.arrange_workspace(workspace_id, layout);

        let workspace = board.workspace(workspace_id).expect("workspace");
        assert_eq!(workspace.layout, Some(layout));
        let first_panel = board.panel(first).expect("first panel");
        let second_panel = board.panel(second).expect("second panel");
        for (name, panel) in [("first", first_panel), ("second", second_panel)] {
            assert!(
                vec2_eq(panel.layout.size, expected_size),
                "{layout:?} should fit the {name} panel to the content area as {expected_size:?}, got {:?}",
                panel.layout.size
            );
        }
        assert!(
            vec2_eq(first_panel.layout.position, content_min),
            "{layout:?} should anchor the first panel at the workspace origin, got {:?}",
            first_panel.layout.position
        );
        let expected_second = match layout {
            WorkspaceLayout::Rows => [content_min[0], content_min[1] + expected_size[1] + TILE_GAP],
            WorkspaceLayout::Columns | WorkspaceLayout::Grid => {
                [content_min[0] + expected_size[0] + TILE_GAP, content_min[1]]
            }
        };
        assert!(
            vec2_eq(second_panel.layout.position, expected_second),
            "{layout:?} should place the second panel at {expected_second:?}, got {:?}",
            second_panel.layout.position
        );
    }
}

#[test]
fn arranging_preset_pushes_neighbor_workspace_on_frame_growth() {
    let mut board = Board::new();
    let left = board.create_workspace_at("left", [0.0, 40.0]);

    let first = board
        .create_panel(editor_panel_options(), left)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), left)
        .expect("second panel should spawn");

    // Stack both panels on the same spot: the content area is one panel wide,
    // so the fitted Columns width clamps to the default size and the
    // arrangement grows the frame toward the neighbor.
    assert!(board.move_panel(first, [40.0, 80.0]));
    assert!(board.move_panel(second, [40.0, 80.0]));

    let right = board.create_workspace_at("right", [630.0, 40.0]);
    board
        .create_panel(editor_panel_options(), right)
        .expect("neighbor panel should spawn");

    let right_before = board.workspace(right).expect("right workspace").position;
    assert!(
        vec2_eq(right_before, [630.0, 40.0]),
        "test setup must leave the neighbor at its explicit position, got {right_before:?}"
    );

    board.arrange_workspace(left, WorkspaceLayout::Columns);

    let right_after = board.workspace(right).expect("right workspace").position;
    assert!(
        right_after[0] > right_before[0],
        "expected the neighbor to be pushed right from {right_before:?}, got {right_after:?}"
    );
    assert!(
        (right_after[1] - right_before[1]).abs() <= f32::EPSILON,
        "expected the neighbor y to stay at {}, got {}",
        right_before[1],
        right_after[1],
    );
}

#[test]
fn switching_between_presets_still_rearranges() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("switch");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let workspace = board.workspace(workspace_id).expect("workspace");
    assert_eq!(workspace.layout, Some(WorkspaceLayout::Rows));
    let origin = workspace.position;
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP
        ]
    ));
}

#[test]
fn hidden_panels_leave_layout_and_focus_and_showing_focuses_them() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("visibility");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    assert_eq!(board.focused, Some(second));

    assert!(board.set_panel_visible(second, false));
    assert!(!board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(first));
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [
            board.workspace(workspace_id).expect("workspace").position[0] + WS_INNER_PAD,
            board.workspace(workspace_id).expect("workspace").position[1] + WS_INNER_PAD,
        ],
    ));

    assert!(board.set_panel_visible(second, true));
    assert!(board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(second));
    assert!(board.workspace_bounds(workspace_id).is_some());

    assert!(board.set_panel_visible(second, false));
    board.focus(second);
    assert!(board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(second));
}

#[test]
fn deferred_cloud_creation_does_not_reflow_or_move_neighbors_before_membership() {
    for preset in WorkspaceLayout::ALL {
        let mut board = Board::new();
        let workspace = board.create_workspace_at("Cloud", [0.0, 0.0]);
        let ordinary = board.create_panel(editor_panel_options(), workspace).unwrap();
        board.arrange_workspace(workspace, preset);
        let neighbor = board.create_workspace_at("Neighbor", [900.0, 0.0]);
        let other = board.create_panel(editor_panel_options(), neighbor).unwrap();
        let before = [ordinary, other].map(|id| board.panel(id).unwrap().layout.position);
        let origin = board.workspace(neighbor).unwrap().position;
        let mut options = editor_panel_options();
        options.position = Some([800.0, 40.0]);
        let child = board
            .create_panel_preserving_workspace_layout(options, workspace)
            .unwrap();
        assert_eq!(board.workspace(workspace).unwrap().layout, Some(preset));
        assert_eq!(
            board.workspace(neighbor).unwrap().position.map(f32::to_bits),
            origin.map(f32::to_bits)
        );
        assert_eq!(
            [ordinary, other].map(|id| board.panel(id).unwrap().layout.position),
            before
        );
        assert_eq!(
            board.panel(child).unwrap().layout.position.map(f32::to_bits),
            [800.0_f32, 40.0].map(f32::to_bits)
        );
    }
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn registering_clouds_publishes_all_members_before_reflow_and_is_idempotent() {
    use crate::cloud_panel::{CloudGroup, CloudGroups};
    let mut board = Board::new();
    let ws = board.create_workspace("Clouds");
    let ids: Vec<_> = (0..3)
        .map(|_| board.create_panel(editor_panel_options(), ws).unwrap())
        .collect();
    let local = board.workspace(ws).unwrap().local_id.clone();
    let mut groups = CloudGroups::default();
    for (index, id) in (0_u16..).zip(&ids[..2]) {
        let mut group = CloudGroup::new(
            u32::from(index),
            "Cloud".into(),
            local.clone(),
            std::path::PathBuf::new(),
            [f32::from(index) * 1600.0, 1000.0],
        );
        group.attach(&mut board, *id);
        groups.0.push(group);
    }
    assert!(
        board.cloud_groups.0.is_empty(),
        "standalone attachment does not register a cloud"
    );
    board.workspace_mut(ws).unwrap().layout = Some(WorkspaceLayout::Columns);
    groups.reconcile(&mut board);
    assert_eq!(board.cloud_groups.0.len(), 2);
    assert!(ids[..2].iter().all(|id| board.cloud_groups.contains_panel(&board, *id)));
    let before: Vec<_> = board
        .panels
        .iter()
        .map(|p| (p.layout.position, p.layout.size))
        .collect();
    groups.reconcile(&mut board);
    assert_eq!(
        board
            .panels
            .iter()
            .map(|p| (p.layout.position, p.layout.size))
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(board.workspace(ws).unwrap().layout, Some(WorkspaceLayout::Columns));
    assert_eq!(board.cloud_groups.0.len(), 2);
}
