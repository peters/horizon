use std::path::PathBuf;

use super::{CloudGroup, CloudGroups};
use crate::board::panel_visual_rect;
use crate::{
    Board, CanvasViewState, PanelId, PanelKind, PanelOptions, RuntimeState, WindowConfig, WorkspaceId, WorkspaceLayout,
};

fn usage_panel(position: [f32; 2], size: [f32; 2]) -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Usage,
        position: Some(position),
        size: Some(size),
        ..PanelOptions::default()
    }
}

fn layouts(board: &Board) -> Vec<([f32; 2], [f32; 2])> {
    board
        .panels
        .iter()
        .map(|panel| (panel.layout.position, panel.layout.size))
        .collect()
}

fn origins(board: &Board) -> Vec<[f32; 2]> {
    board.workspaces.iter().map(|workspace| workspace.position).collect()
}

fn occupied(board: &Board, id: PanelId) -> [f32; 4] {
    let panel = board.panel(id).expect("panel");
    panel_visual_rect(panel.layout.position, panel.layout.size)
}

fn placed_rect(group: &CloudGroup, board: &Board) -> [f32; 4] {
    let (min, max) = group.placed_overview_bounds(board);
    [min[0], min[1], max[0], max[1]]
}

fn overlaps(left: [f32; 4], right: [f32; 4]) -> bool {
    left[0] < right[2] && right[0] < left[2] && left[1] < right[3] && right[1] < left[3]
}

fn assert_same(actual: [f32; 2], expected: [f32; 2]) {
    assert!(
        actual
            .into_iter()
            .zip(expected)
            .all(|(left, right)| (left - right).abs() <= f32::EPSILON),
        "{actual:?} != {expected:?}"
    );
}

fn place(groups: &mut CloudGroups, board: &mut Board, workspace: &str, issue: u32) -> CloudGroup {
    let position = groups.next_position(workspace, board);
    let mut group = CloudGroup::new(
        issue,
        format!("Cloud {issue}"),
        workspace.to_string(),
        PathBuf::new(),
        position,
    );
    group.reconcile(board);
    group
}

fn expected_top(board: &Board, workspace: WorkspaceId, existing: &[CloudGroup]) -> f32 {
    let origin_y = board.workspace(workspace).expect("workspace").position[1];
    let mut bottom = origin_y + 80.0;
    for panel in board.panels.iter().filter(|panel| panel.workspace_id == workspace) {
        bottom = bottom.max(panel_visual_rect(panel.layout.position, panel.layout.size)[3]);
    }
    for group in existing {
        bottom = bottom.max(group.placed_overview_bounds(board).1[1]);
    }
    bottom + 48.0
}

fn assert_placement(board: &Board, workspace: WorkspaceId, group: &CloudGroup, existing: &[CloudGroup]) {
    let origin = board.workspace(workspace).expect("workspace").position;
    let cloud = placed_rect(group, board);
    assert!(
        (group.position[0] - origin[0] - 24.0).abs() < 0.01,
        "cloud x {:?} is not workspace-relative",
        group.position
    );
    let top = expected_top(board, workspace, existing);
    assert!(
        (group.position[1] - top).abs() < 0.01,
        "cloud y {:?} expected {top}",
        group.position
    );
    for panel in board.panels.iter().filter(|panel| panel.workspace_id == workspace) {
        let rect = panel_visual_rect(panel.layout.position, panel.layout.size);
        assert!(!overlaps(cloud, rect), "cloud {cloud:?} covers panel {rect:?}");
    }
    for other in existing {
        let rect = placed_rect(other, board);
        assert!(!overlaps(cloud, rect), "cloud {cloud:?} covers cloud {rect:?}");
    }
    assert!(group.panels.is_empty());
}

#[test]
fn first_cloud_clears_ordinary_panel_bounds_without_moving_them() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Local", [500.0, -40.0]);
    let origin = board.workspace(workspace).expect("workspace").position;
    let ids = [
        board
            .create_panel(
                usage_panel([origin[0] + 20.0, origin[1] + 30.0], [480.0, 260.0]),
                workspace,
            )
            .expect("panel"),
        board
            .create_panel(
                usage_panel([origin[0] + 560.0, origin[1] + 30.0], [300.0, 180.0]),
                workspace,
            )
            .expect("panel"),
        board
            .create_panel(
                usage_panel([origin[0] - 80.0, origin[1] + 10.0], [220.0, 640.0]),
                workspace,
            )
            .expect("panel"),
    ];
    let before = layouts(&board);
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let mut groups = CloudGroups::default();
    let group = place(&mut groups, &mut board, &local, 1);
    assert_eq!(layouts(&board), before);
    assert_placement(&board, workspace, &group, &[]);
    let lowest = ids
        .into_iter()
        .map(|id| occupied(&board, id)[3])
        .fold(f32::MIN, f32::max);
    assert!(group.position[1] >= lowest + 48.0);
    groups.0.push(group);
    groups.adopt_intersecting(&board);
    assert!(groups.0[0].panels.is_empty());
}

#[test]
fn another_cloud_keeps_a_manually_placed_cloud_and_its_panels() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Local", [80.0, 60.0]);
    let origin = board.workspace(workspace).expect("workspace").position;
    board
        .create_panel(
            usage_panel([origin[0] + 40.0, origin[1] + 50.0], [640.0, 420.0]),
            workspace,
        )
        .expect("panel");
    board.workspace_mut(workspace).expect("workspace").layout = None;
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let mut groups = CloudGroups::default();
    let group = place(&mut groups, &mut board, &local, 1);
    groups.0.push(group);
    board.cloud_groups.clone_from(&groups);
    let before = layouts(&board);
    groups.0[0].translate(&mut board, [220.0, 90.0]);
    let parked = groups.0[0].position;
    let second = place(&mut groups, &mut board, &local, 2);
    assert_same(groups.0[0].position, parked);
    assert_eq!(layouts(&board), before);
    assert_eq!(board.workspace(workspace).expect("workspace").layout, None);
    assert_placement(&board, workspace, &second, &groups.0);
}

#[test]
fn placement_follows_a_translated_workspace_before_the_cloud_reconciles() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Local", [0.0, 0.0]);
    board
        .create_panel(usage_panel([30.0, 40.0], [520.0, 300.0]), workspace)
        .expect("panel");
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let mut groups = CloudGroups::default();
    let group = place(&mut groups, &mut board, &local, 1);
    groups.0.push(group);
    board.cloud_groups.clone_from(&groups);
    assert!(board.translate_workspace(workspace, [480.0, -150.0]));
    let stale = groups.0[0].position;
    let before = layouts(&board);
    let second = place(&mut groups, &mut board, &local, 2);
    assert_same(groups.0[0].position, stale);
    assert_eq!(layouts(&board), before);
    assert_placement(&board, workspace, &second, &groups.0);
    groups.0[0].reconcile(&mut board);
    let drawn = placed_rect(&groups.0[0], &board);
    let created = placed_rect(&second, &board);
    assert!(
        !overlaps(drawn, created),
        "reconciled cloud {drawn:?} meets new cloud {created:?}"
    );
    for panel in board.panels.iter().filter(|panel| panel.workspace_id == workspace) {
        let rect = panel_visual_rect(panel.layout.position, panel.layout.size);
        assert!(!overlaps(created, rect));
        assert!(!overlaps(drawn, rect));
    }
}

#[test]
fn resized_panel_bounds_push_the_next_cloud_down() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Local", [10.0, 20.0]);
    let origin = board.workspace(workspace).expect("workspace").position;
    let id = board
        .create_panel(
            usage_panel([origin[0] + 24.0, origin[1] + 24.0], [400.0, 200.0]),
            workspace,
        )
        .expect("panel");
    board.workspace_mut(workspace).expect("workspace").layout = None;
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let before_resize = place(&mut CloudGroups::default(), &mut board, &local, 1).position;
    assert!(board.resize_panel(id, [900.0, 760.0]));
    let moved = layouts(&board);
    let group = place(&mut CloudGroups::default(), &mut board, &local, 2);
    assert_eq!(layouts(&board), moved);
    assert!(group.position[1] > before_resize[1]);
    assert_placement(&board, workspace, &group, &[]);
    assert!(group.position[1] >= occupied(&board, id)[3] + 48.0);
}

#[test]
fn other_workspaces_do_not_change_placement_or_move() {
    let mut board = Board::new();
    let busy = board.create_workspace_at("Busy", [0.0, 0.0]);
    let quiet = board.create_workspace_at("Quiet", [6000.0, 40.0]);
    let busy_origin = board.workspace(busy).expect("busy").position;
    let quiet_origin = board.workspace(quiet).expect("quiet").position;
    board
        .create_panel(
            usage_panel([busy_origin[0] + 20.0, busy_origin[1] + 20.0], [700.0, 1600.0]),
            busy,
        )
        .expect("busy panel");
    let quiet_panel = board
        .create_panel(
            usage_panel([quiet_origin[0] + 20.0, quiet_origin[1] + 20.0], [360.0, 140.0]),
            quiet,
        )
        .expect("quiet panel");
    let local = board.workspace(quiet).expect("quiet").local_id.clone();
    let mut busy_cloud = CloudGroup::new(
        7,
        "Busy".into(),
        board.workspace(busy).expect("busy").local_id.clone(),
        PathBuf::new(),
        busy_origin,
    );
    busy_cloud.reconcile(&mut board);
    let mut groups = CloudGroups(vec![busy_cloud]);
    let before_layouts = layouts(&board);
    let before_origins = origins(&board);
    let group = place(&mut groups, &mut board, &local, 1);
    assert_eq!(layouts(&board), before_layouts);
    assert_eq!(origins(&board), before_origins);
    assert_same(groups.0[0].position, busy_origin);
    assert_placement(&board, quiet, &group, &[]);
    let foreign_bottom = board
        .panels
        .iter()
        .find(|panel| panel.workspace_id == busy)
        .map(|panel| panel_visual_rect(panel.layout.position, panel.layout.size)[3])
        .expect("busy panel");
    assert!(group.position[1] < foreign_bottom);
    assert!(group.position[1] >= occupied(&board, quiet_panel)[3] + 48.0);
}

#[test]
fn grid_preset_panels_stay_put_when_the_cloud_lands_below_them() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Grid", [140.0, 90.0]);
    assert_eq!(
        board.workspace(workspace).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    for _ in 0..3 {
        board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("panel");
    }
    assert_eq!(
        board.workspace(workspace).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let before = layouts(&board);
    let mut groups = CloudGroups::default();
    let group = place(&mut groups, &mut board, &local, 1);
    groups.0.push(group);
    let frame = board.workspace_frame_rect(workspace);
    board.cloud_groups.clone_from(&groups);
    board.reapply_workspace_layout_after(workspace, frame);
    assert_eq!(layouts(&board), before);
    assert_eq!(
        board.workspace(workspace).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    assert_placement(&board, workspace, &board.cloud_groups.0[0], &[]);
}

#[test]
fn placement_survives_restart_and_stays_clear() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("Local", [320.0, 180.0]);
    let origin = board.workspace(workspace).expect("workspace").position;
    board
        .create_panel(
            usage_panel([origin[0] + 16.0, origin[1] + 28.0], [540.0, 360.0]),
            workspace,
        )
        .expect("panel");
    board
        .create_panel(
            usage_panel([origin[0] + 600.0, origin[1] + 80.0], [280.0, 220.0]),
            workspace,
        )
        .expect("panel");
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let mut groups = CloudGroups::default();
    let group = place(&mut groups, &mut board, &local, 1);
    groups.0.push(group);
    assert!(board.translate_workspace(workspace, [60.0, 25.0]));
    groups.0[0].reconcile(&mut board);
    board.cloud_groups.clone_from(&groups);
    let saved_layouts = layouts(&board);
    let saved_cloud = board.cloud_groups.0[0].position;
    let snapshot = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let encoded = serde_json::to_vec(&snapshot).expect("encode runtime");
    let decoded: RuntimeState = serde_json::from_slice(&encoded).expect("decode runtime");
    let mut restored = Board::from_runtime_state(&decoded).expect("restore");
    assert_eq!(layouts(&restored), saved_layouts);
    assert_same(restored.cloud_groups.0[0].position, saved_cloud);
    let mut group = restored.cloud_groups.0[0].clone();
    group.reconcile(&mut restored);
    assert_same(group.position, saved_cloud);
    restored.cloud_groups.0[0] = group;
    assert_same(restored.cloud_groups.0[0].position, saved_cloud);
    assert_eq!(layouts(&restored), saved_layouts);
    let workspace = restored.workspace_id_by_local_id(&local).expect("workspace");
    assert_placement(&restored, workspace, &restored.cloud_groups.0[0], &[]);
}

#[test]
fn empty_workspace_keeps_the_default_cloud_offset() {
    for origin in [[0.0, 0.0], [500.0, -100.0], [-40.0, 250.0]] {
        let mut board = Board::new();
        let workspace = board.create_workspace_at("Empty", origin);
        let local = board.workspace(workspace).expect("workspace").local_id.clone();
        let relative = CloudGroups::default().next_position(&local, &board);
        assert!((relative[0] - 24.0).abs() < 0.01);
        assert!((relative[1] - 128.0).abs() < 0.01);
        let group = place(&mut CloudGroups::default(), &mut board, &local, 1);
        assert!((group.position[0] - origin[0] - 24.0).abs() < 0.01);
        assert!((group.position[1] - origin[1] - 128.0).abs() < 0.01);
        assert!(board.panels.is_empty());
    }
}
