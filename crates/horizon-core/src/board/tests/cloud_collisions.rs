use std::path::PathBuf;

use super::*;
use crate::board::{Board, vec2_eq};
use crate::cloud_panel::{CHILD_SIZE, CloudGroup, HEADER, PAD};
use crate::layout::TILE_GAP;
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

fn panel_at(board: &mut Board, workspace: WorkspaceId, position: [f32; 2], size: [f32; 2]) -> PanelId {
    board
        .create_panel(
            PanelOptions {
                position: Some(position),
                size: Some(size),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("panel should spawn")
}

fn cloud_at(board: &Board, workspace: WorkspaceId, position: [f32; 2]) -> CloudGroup {
    let local_id = board.workspace(workspace).expect("workspace").local_id.clone();
    CloudGroup::new(101, "Cloud".into(), local_id, PathBuf::new(), position)
}

fn rect(min: [f32; 2], max: [f32; 2]) -> [f32; 4] {
    [min[0], min[1], max[0], max[1]]
}

fn panel_rect(board: &Board, id: PanelId) -> [f32; 4] {
    let layout = &board.panel(id).expect("panel").layout;
    rect(
        layout.position,
        [layout.position[0] + layout.size[0], layout.position[1] + layout.size[1]],
    )
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

#[test]
fn growing_ordinary_panel_pushes_an_empty_cloud_frame() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let panel = panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let cloud = cloud_at(&board, workspace, [610.0, 120.0]);
    board.cloud_groups.0.push(cloud);

    assert!(board.resize_panel(panel, [760.0, 360.0]));

    let cloud = &board.cloud_groups.0[0];
    assert!(vec2_eq(cloud.position, [60.0 + 760.0 + TILE_GAP, 120.0]));
    let (min, max) = cloud.overview_bounds();
    assert!(!overlaps(panel_rect(&board, panel), rect(min, max)));
}

#[test]
fn pushed_cloud_carries_its_members_and_pushes_panels_beyond_it() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let growing = panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let mut cloud = cloud_at(&board, workspace, [610.0, 120.0]);
    let member = panel_at(&mut board, workspace, [610.0 + PAD, 120.0 + HEADER], CHILD_SIZE);
    cloud.attach(&mut board, member);
    board.cloud_groups.0.push(cloud);
    let beyond_x = board.cloud_groups.0[0].overview_bounds().1[0] + 30.0;
    let beyond = panel_at(&mut board, workspace, [beyond_x, 120.0], [400.0, 300.0]);

    assert!(board.resize_panel(growing, [760.0, 360.0]));

    let shift = 60.0 + 760.0 + TILE_GAP - 610.0;
    let cloud = &board.cloud_groups.0[0];
    assert!(vec2_eq(cloud.position, [610.0 + shift, 120.0]));
    assert!(vec2_eq(
        board.panel(member).expect("member").layout.position,
        [610.0 + PAD + shift, 120.0 + HEADER]
    ));
    let (min, max) = cloud.overview_bounds();
    let pushed_x = board.panel(beyond).expect("beyond").layout.position[0];
    assert!((pushed_x - (max[0] + TILE_GAP)).abs() <= f32::EPSILON);
    for id in [growing, beyond] {
        assert!(!overlaps(panel_rect(&board, id), rect(min, max)));
    }

    let cloud_before = board.cloud_groups.0[0].position;
    assert!(board.resize_panel(member, [CHILD_SIZE[0] + 40.0, CHILD_SIZE[1]]));
    assert!(vec2_eq(board.cloud_groups.0[0].position, cloud_before));
}

#[test]
fn bodies_pushed_onto_each_other_are_separated() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let growing = panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let panel = panel_at(&mut board, workspace, [560.0, 140.0], [300.0, 200.0]);
    let cloud = cloud_at(&board, workspace, [900.0, 300.0]);
    board.cloud_groups.0.push(cloud);

    assert!(board.resize_panel(growing, [900.0, 360.0]));

    let (min, max) = board.cloud_groups.0[0].overview_bounds();
    let cloud = rect(min, max);
    let growing = panel_rect(&board, growing);
    let panel = panel_rect(&board, panel);
    assert!(!overlaps(growing, cloud));
    assert!(!overlaps(growing, panel));
    assert!(
        !overlaps(panel, cloud),
        "panel {panel:?} still overlaps cloud {cloud:?}"
    );
}

fn neighbour_clears(board: &Board, neighbour: WorkspaceId, cloud: ([f32; 2], [f32; 2])) -> bool {
    let frame = board.workspace_frame_rect(neighbour).expect("neighbour frame");
    !overlaps(frame, rect(cloud.0, cloud.1))
}

#[test]
fn neighbour_workspace_is_pushed_clear_of_a_pushed_cloud() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let growing = panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let cloud = cloud_at(&board, workspace, [610.0, 120.0]);
    board.cloud_groups.0.push(cloud);
    let neighbour = board.create_workspace_at("neighbour", [1600.0, 0.0]);
    panel_at(&mut board, neighbour, [1640.0, 120.0], [420.0, 300.0]);

    assert!(board.resize_panel(growing, [760.0, 360.0]));

    let cloud = board.cloud_groups.0[0].overview_bounds();
    assert!(neighbour_clears(&board, neighbour, cloud));
}

#[test]
fn dragged_workspace_pushes_neighbours_clear_of_its_unreconciled_clouds() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let cloud = cloud_at(&board, workspace, [610.0, 120.0]);
    board.cloud_groups.0.push(cloud);
    let neighbour = board.create_workspace_at("neighbour", [1600.0, 0.0]);
    panel_at(&mut board, neighbour, [1640.0, 120.0], [420.0, 300.0]);

    assert!(board.translate_workspace_with_push(workspace, [300.0, 0.0]));

    let cloud = reconciled_cloud_bounds(&mut board, 0);
    assert!(vec2_eq(cloud.0, [910.0, 120.0]));
    assert!(neighbour_clears(&board, neighbour, cloud));
}

/// Where the UI's next reconcile puts the cloud after workspace moves.
fn reconciled_cloud_bounds(board: &mut Board, index: usize) -> ([f32; 2], [f32; 2]) {
    let mut cloud = board.cloud_groups.0[index].clone();
    cloud.reconcile(board);
    cloud.overview_bounds()
}

#[test]
fn pushed_workspace_carries_its_clouds_into_the_cascade() {
    let mut board = Board::new();
    let first = board.create_workspace_at("first", [0.0, 0.0]);
    let growing = panel_at(&mut board, first, [60.0, 120.0], [480.0, 360.0]);
    let second = board.create_workspace_at("second", [700.0, 0.0]);
    panel_at(&mut board, second, [740.0, 120.0], [300.0, 200.0]);
    let mut cloud = cloud_at(&board, second, [400.0, 120.0]);
    cloud.reconcile(&mut board);
    board.cloud_groups.0.push(cloud);
    let third = board.create_workspace_at("third", [2200.0, 0.0]);
    panel_at(&mut board, third, [2240.0, 120.0], [420.0, 300.0]);

    assert!(board.resize_panel(growing, [900.0, 360.0]));

    let cloud = reconciled_cloud_bounds(&mut board, 0);
    assert!(cloud.0[0] > 1100.0, "the second workspace should have been pushed");
    assert!(neighbour_clears(&board, third, cloud));
}

#[test]
fn aligned_workspaces_keep_clear_of_clouds() {
    let mut board = Board::new();
    let first = board.create_workspace_at("first", [0.0, 0.0]);
    panel_at(&mut board, first, [60.0, 120.0], [480.0, 360.0]);
    let cloud = cloud_at(&board, first, [610.0, 120.0]);
    board.cloud_groups.0.push(cloud);
    let second = board.create_workspace_at("second", [0.0, 1200.0]);
    panel_at(&mut board, second, [40.0, 1280.0], [420.0, 300.0]);

    assert!(board.align_workspaces_horizontally(&[first, second]).is_some());

    let cloud = reconciled_cloud_bounds(&mut board, 0);
    assert!(neighbour_clears(&board, second, cloud));
}

#[test]
fn panel_pushes_a_cloud_from_where_its_moved_workspace_places_it() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let growing = panel_at(&mut board, workspace, [60.0, 120.0], [480.0, 360.0]);
    let cloud = cloud_at(&board, workspace, [610.0, 120.0]);
    board.cloud_groups.0.push(cloud);
    assert!(board.translate_workspace(workspace, [300.0, 0.0]));

    assert!(board.resize_panel(growing, [600.0, 360.0]));

    let panel_right = panel_rect(&board, growing)[2];
    let cloud = reconciled_cloud_bounds(&mut board, 0);
    assert!(
        (cloud.0[0] - (panel_right + TILE_GAP)).abs() <= 0.01,
        "cloud at {cloud:?}"
    );
}
