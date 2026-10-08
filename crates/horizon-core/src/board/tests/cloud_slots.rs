//! A deployed cloud takes one slot of its workspace's preset, like the panels beside it.
use std::path::PathBuf;

use super::*;
use crate::board::{Board, WorkspaceLayout};
use crate::cloud_panel::{CHILD_SIZE, CloudGroup, CloudLaunch, SLOT_MIN_MEMBER};
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

fn launch() -> CloudLaunch {
    let config = crate::cloud_panel::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .expect("config");
    CloudLaunch {
        deployment_started: true,
        id: "slot-fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
        placement: crate::cloud_panel::Placement::default(),
    }
}

fn panel(board: &mut Board, workspace: WorkspaceId) -> PanelId {
    board
        .create_panel(editor_panel_options(), workspace)
        .expect("panel should spawn")
}

/// A Grid workspace with two panels, then a deployed cloud holding `members` panels.
fn desk(members: usize) -> (Board, WorkspaceId, [PanelId; 2], Vec<PanelId>) {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
    let panels = [panel(&mut board, workspace), panel(&mut board, workspace)];
    let local = board.workspace(workspace).expect("workspace").local_id.clone();
    let mut cloud = CloudGroup::new(101, "Cloud".into(), local, PathBuf::new(), [2000.0, 0.0]);
    cloud.remote = Some(launch());
    cloud.environment.id = "slot-fixture".into();
    let mut inside = Vec::new();
    for _ in 0..members {
        let id = board
            .create_panel(
                PanelOptions {
                    size: Some(CHILD_SIZE),
                    ..editor_panel_options()
                },
                workspace,
            )
            .expect("member");
        cloud.attach(&mut board, id);
        inside.push(id);
    }
    if members > 0 {
        cloud.set_layout(&mut board, Some(WorkspaceLayout::Grid));
    }
    board.cloud_groups.0.push(cloud);
    board.arrange_workspace(workspace, WorkspaceLayout::Grid);
    (board, workspace, panels, inside)
}

fn rect(position: [f32; 2], size: [f32; 2]) -> [f32; 4] {
    [position[0], position[1], position[0] + size[0], position[1] + size[1]]
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] - 0.01 && b[0] < a[2] - 0.01 && a[1] < b[3] - 0.01 && b[1] < a[3] - 0.01
}

fn near(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05
}

fn cloud(board: &Board) -> &CloudGroup {
    &board.cloud_groups.0[0]
}

fn assert_members_inside(board: &Board, members: &[PanelId]) {
    let frame = rect(cloud(board).position, cloud(board).size);
    for id in members {
        let layout = &board.panel(*id).expect("member").layout;
        let member = rect(layout.position, layout.size);
        assert!(
            member[0] >= frame[0] - 0.05
                && member[1] >= frame[1] - 0.05
                && member[2] <= frame[2] + 0.05
                && member[3] <= frame[3] + 0.05,
            "{member:?} outside {frame:?}"
        );
    }
}

fn assert_no_overlap(board: &Board, panels: &[PanelId]) {
    let frame = rect(cloud(board).position, cloud(board).size);
    let rects: Vec<_> = panels
        .iter()
        .map(|id| {
            let layout = &board.panel(*id).expect("panel").layout;
            rect(layout.position, layout.size)
        })
        .collect();
    for (index, a) in rects.iter().enumerate() {
        assert!(!overlaps(*a, frame), "{a:?} over the cloud {frame:?}");
        for b in &rects[index + 1..] {
            assert!(!overlaps(*a, *b), "{a:?} over {b:?}");
        }
    }
}

#[test]
fn a_deployed_cloud_takes_a_grid_slot_the_size_of_its_neighbours() {
    let (board, _, panels, members) = desk(1);
    let size = board.panel(panels[0]).expect("panel").layout.size;
    assert!(near(board.panel(panels[1]).expect("panel").layout.size, size));
    assert!(near(cloud(&board).size, size), "{:?} vs {size:?}", cloud(&board).size);
    assert_members_inside(&board, &members);
    assert_no_overlap(&board, &panels);
    // Grid of three: two slots on the first row, the cloud first on the second.
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert!((cloud(&board).position[0] - first[0]).abs() < 0.05);
    assert!(cloud(&board).position[1] > first[1] + size[1]);
}

#[test]
fn resizing_a_panel_resizes_the_cloud_slot_and_its_members() {
    let (mut board, _, panels, members) = desk(2);
    let before = board.panel(members[0]).expect("member").layout.size;
    let size = [900.0, 700.0];
    assert!(board.resize_panel(panels[0], size));
    assert!(near(cloud(&board).size, size), "{:?}", cloud(&board).size);
    assert!(near(board.panel(panels[1]).expect("panel").layout.size, size));
    let after = board.panel(members[0]).expect("member").layout.size;
    assert!(after[0] > before[0] && after[1] > before[1], "{before:?} -> {after:?}");
    assert_members_inside(&board, &members);
    assert_no_overlap(&board, &panels);
}

#[test]
fn resizing_the_cloud_slot_resizes_every_panel() {
    let (mut board, _, panels, members) = desk(1);
    let size = [1000.0, 760.0];
    assert!(board.resize_cloud_slot("slot-fixture", size));
    for id in panels {
        assert!(near(board.panel(id).expect("panel").layout.size, size));
    }
    assert!(near(cloud(&board).size, size));
    assert_members_inside(&board, &members);
    assert_no_overlap(&board, &panels);
}

#[test]
fn reapplying_the_preset_leaves_every_slot_where_it_was() {
    let (mut board, workspace, panels, members) = desk(2);
    let snapshot = |board: &Board| {
        let mut bits: Vec<u32> = cloud(board)
            .position
            .iter()
            .chain(cloud(board).size.iter())
            .map(|value| value.to_bits())
            .collect();
        for id in panels.iter().chain(&members) {
            let layout = &board.panel(*id).expect("panel").layout;
            bits.extend(
                layout
                    .position
                    .iter()
                    .chain(layout.size.iter())
                    .map(|value| value.to_bits()),
            );
        }
        bits
    };
    let first = snapshot(&board);
    board.reapply_workspace_layout_if_set(workspace);
    board.reapply_workspace_layout_if_set(workspace);
    assert_eq!(snapshot(&board), first);
}

#[test]
fn a_cloud_that_cannot_shrink_raises_the_size_every_slot_shares() {
    let (mut board, _, panels, members) = desk(4);
    assert!(board.resize_panel(panels[0], [320.0, 220.0]));
    let size = board.panel(panels[0]).expect("panel").layout.size;
    assert!(near(board.panel(panels[1]).expect("panel").layout.size, size));
    assert!(near(cloud(&board).size, size), "{:?} vs {size:?}", cloud(&board).size);
    for id in &members {
        let member = board.panel(*id).expect("member").layout.size;
        assert!(member[0] >= SLOT_MIN_MEMBER[0] - 0.05 && member[1] >= SLOT_MIN_MEMBER[1] - 0.05);
    }
    assert!(
        size[0] > 320.0 && size[1] > 220.0,
        "the cloud floor lifted every slot: {size:?}"
    );
    assert_members_inside(&board, &members);
    assert_no_overlap(&board, &panels);
}

#[test]
fn dragging_a_panel_onto_the_cloud_swaps_their_slots_and_the_order_is_saved() {
    let (mut board, workspace, panels, members) = desk(1);
    let cloud_slot = cloud(&board).position;
    let panel_slot = board.panel(panels[1]).expect("panel").layout.position;
    let size = board.panel(panels[1]).expect("panel").layout.size;
    assert!(board.reorder_arranged_slot(panels[1], cloud_slot));
    assert!(near(board.panel(panels[1]).expect("panel").layout.position, cloud_slot));
    assert!(near(cloud(&board).position, panel_slot));
    assert_members_inside(&board, &members);
    let order = &board.workspace(workspace).expect("workspace").panels;
    let member_at = order.iter().position(|id| *id == members[0]).expect("member");
    let moved_at = order.iter().position(|id| *id == panels[1]).expect("panel");
    assert!(member_at < moved_at, "the cloud's slot is stored by its member's place");
    // Dragging the cloud by its header onto the panel's slot restores the first order.
    assert!(board.swap_cloud_slot_at("slot-fixture", [cloud_slot[0] + 10.0, cloud_slot[1] + 10.0]));
    assert!(near(cloud(&board).position, cloud_slot));
    assert!(near(board.panel(panels[1]).expect("panel").layout.position, panel_slot));
    assert!(near(board.panel(panels[1]).expect("panel").layout.size, size));
}

#[test]
fn a_collapsed_cloud_gives_its_slot_back() {
    let (mut board, workspace, panels, _) = desk(1);
    let mut group = board.cloud_groups.0[0].clone();
    group.set_collapsed(&mut board, true);
    board.cloud_groups.0[0] = group.clone();
    board.reapply_workspace_layout_if_set(workspace);
    assert_eq!(board.arranged_slots(workspace).len(), 2);
    group.set_collapsed(&mut board, false);
    board.cloud_groups.0[0] = group;
    board.reapply_workspace_layout_if_set(workspace);
    assert_eq!(board.arranged_slots(workspace).len(), 3);
    assert_no_overlap(&board, &panels);
}

#[test]
fn a_freeform_workspace_leaves_the_cloud_out_of_any_slot() {
    let (mut board, workspace, panels, _) = desk(1);
    board.clear_workspace_layout(workspace);
    let before = cloud(&board).position;
    assert!(!board.cloud_takes_slot(cloud(&board)));
    let _ = board.move_panel(panels[0], [5000.0, 5000.0]);
    assert!(near(cloud(&board).position, before));
}

#[test]
fn a_restored_cloud_saved_outside_its_slot_takes_it_again() {
    let (mut board, _, _, members) = desk(1);
    let slot = cloud(&board).position;
    let member = board.panel(members[0]).expect("member").layout.position;
    let mut moved = board.cloud_groups.0[0].clone();
    moved.shift_in_slot(&mut board, [3000.0, 500.0]);
    board.cloud_groups.0[0] = moved;
    let state = crate::RuntimeState::from_board(
        &board,
        crate::WindowConfig::default(),
        crate::CanvasViewState::default(),
    );
    let restored = Board::from_runtime_state(&state).expect("restore");
    assert!(
        near(restored.cloud_groups.0[0].position, slot),
        "{:?}",
        restored.cloud_groups.0[0].position
    );
    let local = &board.panel(members[0]).expect("member").local_id;
    let id = restored.panel_id_by_local_id(local).expect("restored member");
    assert!(near(restored.panel(id).expect("member").layout.position, member));
}

#[test]
fn an_empty_cloud_keeps_the_slot_it_was_dragged_to() {
    let (mut board, workspace, panels, _) = desk(0);
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert!(board.swap_cloud_slot_at("slot-fixture", [first[0] + 10.0, first[1] + 10.0]));
    assert!(
        near(cloud(&board).position, first),
        "the empty cloud took the first slot"
    );
    assert_eq!(cloud(&board).slot, Some(0));
    board.reapply_workspace_layout_if_set(workspace);
    assert!(near(cloud(&board).position, first), "and keeps it");
    let state = crate::RuntimeState::from_board(
        &board,
        crate::WindowConfig::default(),
        crate::CanvasViewState::default(),
    );
    let restored = Board::from_runtime_state(&state).expect("restore");
    assert!(near(restored.cloud_groups.0[0].position, first), "after a restart too");
}

#[test]
fn a_cloud_keeps_its_slot_when_its_last_member_closes() {
    let (mut board, workspace, panels, members) = desk(1);
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert!(board.swap_cloud_slot_at("slot-fixture", [first[0] + 10.0, first[1] + 10.0]));
    assert!(near(cloud(&board).position, first));
    board.close_panel(members[0]);
    let mut group = board.cloud_groups.0[0].clone();
    group.reconcile(&mut board);
    board.reapply_workspace_layout_if_set(workspace);
    assert!(near(cloud(&board).position, first), "{:?}", cloud(&board).position);
}

#[test]
fn a_collapsed_cloud_returns_to_its_slot_after_its_neighbours_swap() {
    let (mut board, workspace, panels, members) = desk(1);
    let second = board.panel(panels[1]).expect("panel").layout.position;
    assert!(board.swap_cloud_slot_at("slot-fixture", [second[0] + 10.0, second[1] + 10.0]));
    let middle = cloud(&board).position;
    let mut group = board.cloud_groups.0[0].clone();
    group.set_collapsed(&mut board, true);
    board.cloud_groups.0[0] = group.clone();
    board.reapply_workspace_layout_if_set(workspace);
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert!(board.reorder_arranged_slot(panels[1], first));
    let order = &board.workspace(workspace).expect("workspace").panels;
    assert_eq!(
        order.iter().position(|id| *id == members[0]),
        Some(1),
        "the hidden member kept its place"
    );
    group.set_collapsed(&mut board, false);
    board.cloud_groups.0[0] = group;
    board.reapply_workspace_layout_if_set(workspace);
    assert!(
        near(cloud(&board).position, middle),
        "{:?} vs {middle:?}",
        cloud(&board).position
    );
}

#[test]
fn resizing_a_member_of_a_slotted_cloud_resizes_every_slot() {
    let (mut board, _, panels, members) = desk(2);
    let mut groups = crate::cloud_panel::CloudGroups(board.cloud_groups.0.clone());
    let before = cloud(&board).size;
    assert!(groups.resize_panel(&mut board, members[0], [520.0, 380.0]));
    let size = cloud(&board).size;
    assert!(size[0] > before[0], "the cloud grew: {before:?} -> {size:?}");
    for id in panels {
        assert!(
            near(board.panel(id).expect("panel").layout.size, size),
            "every slot takes the cloud's size"
        );
    }
    assert!(near(groups.0[0].size, size) && near(groups.0[0].position, cloud(&board).position));
    assert_members_inside(&board, &members);
    assert_no_overlap(&board, &panels);
}

#[test]
fn an_empty_cloud_keeps_its_slot_when_it_gains_its_first_member() {
    let (mut board, workspace, panels, _) = desk(0);
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert!(board.swap_cloud_slot_at("slot-fixture", [first[0] + 10.0, first[1] + 10.0]));
    assert!(near(cloud(&board).position, first));
    let member = board
        .create_panel(
            PanelOptions {
                size: Some(CHILD_SIZE),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("member");
    let mut group = board.cloud_groups.0[0].clone();
    group.attach(&mut board, member);
    assert!(
        near(cloud(&board).position, first),
        "{:?} vs {first:?}",
        cloud(&board).position
    );
    assert!(near(group.position, first), "the attaching copy follows");
    assert_members_inside(&board, &[member]);
}

#[test]
fn only_the_unscoped_slot_resize_moves_other_workspaces() {
    let (mut board, workspace, _, _) = desk(1);
    let right = board.workspace_frame_rect(workspace).expect("frame")[2];
    let neighbour = board.create_workspace_at("neighbour", [right + 40.0, 0.0]);
    let _ = panel(&mut board, neighbour);
    let before = board.workspace(neighbour).expect("neighbour").position;
    assert!(board.place_cloud_slot("slot-fixture", [1200.0, 900.0]));
    let after = board.workspace(neighbour).expect("neighbour").position;
    assert!(crate::board::vec2_eq(after, before), "the caller decides");
    assert!(board.resize_cloud_slot("slot-fixture", [1400.0, 1000.0]));
    let pushed = board.workspace(neighbour).expect("neighbour").position;
    assert!(!crate::board::vec2_eq(pushed, before), "everything is in scope");
}

#[test]
fn a_cloud_moves_up_when_the_panels_before_it_close() {
    let (mut board, workspace, panels, _) = desk(0);
    let first = board.panel(panels[0]).expect("panel").layout.position;
    assert_eq!(cloud(&board).slot, Some(2), "the empty cloud follows both panels");
    // The UI keeps a workspace that holds a cloud when its last panel closes.
    board.retain_workspace_when_empty(workspace);
    board.close_panel(panels[0]);
    assert_eq!(cloud(&board).slot, Some(1), "one panel before it is left");
    board.close_panel(panels[1]);
    assert_eq!(cloud(&board).slot, Some(0));
    assert!(
        near(cloud(&board).position, first),
        "no gap where the panels were: {:?}",
        cloud(&board).position
    );
}
