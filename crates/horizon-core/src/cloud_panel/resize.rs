//! Resize a cloud frame from its corner.
use super::{CloudGroup, CloudGroups, HEADER, PAD};
use crate::{Board, WorkspaceLayout, layout};

/// Default frame of a cloud that has no visible members.
#[must_use]
pub(super) const fn default_frame() -> [f32; 2] {
    [super::CHILD_SIZE[0] + PAD * 2.0, super::CHILD_SIZE[1] + HEADER + PAD]
}

impl CloudGroups {
    /// Resize the cloud identified by `issue`.
    ///
    /// A layout preset gives every visible member the same cell size so the
    /// frame stays tight around the arrangement, and will not go below
    /// `min_member`. Manual placement keeps member geometry and will not shrink
    /// through those panels. A collapsed cloud is left unchanged.
    pub fn resize_frame(&mut self, board: &mut Board, issue: u32, size: [f32; 2], min_member: [f32; 2]) -> bool {
        if !size.iter().chain(min_member.iter()).all(|value| value.is_finite()) {
            return false;
        }
        let Some(index) = self.0.iter().position(|group| group.issue == issue) else {
            return false;
        };
        if self.0[index].collapsed {
            return false;
        }
        let before = self.0[index].size;
        let min_member = [min_member[0].max(1.0), min_member[1].max(1.0)];
        self.0[index].apply_frame_size(board, size, min_member);
        self.0[index].reconcile_with_collisions(board, false);
        if self.0[index]
            .size
            .iter()
            .zip(before)
            .any(|(next, previous)| *next > previous)
        {
            self.make_room_with_collisions(board, index, false);
        }
        true
    }
}

impl CloudGroup {
    fn apply_frame_size(&mut self, board: &mut Board, requested: [f32; 2], min_member: [f32; 2]) {
        let visible = visible_members(self, board);
        if let Some(layout) = self.layout
            && !visible.is_empty()
        {
            let cell = cell_filling(layout, visible.len(), requested);
            let cell = [cell[0].max(min_member[0]), cell[1].max(min_member[1])];
            for local in &visible {
                if let Some(panel) = board.panels.iter_mut().find(|panel| &panel.local_id == local) {
                    panel.resize_layout(cell);
                }
            }
            self.arrange(board);
            return;
        }
        let floor = if visible.is_empty() {
            default_frame()
        } else {
            manual_floor(self, board)
        };
        self.size = [requested[0].max(floor[0]), requested[1].max(floor[1])];
    }
}

fn visible_members(group: &CloudGroup, board: &Board) -> Vec<String> {
    group
        .panels
        .iter()
        .filter(|local| {
            board
                .panels
                .iter()
                .any(|panel| &panel.local_id == *local && panel.visible)
        })
        .cloned()
        .collect()
}

fn manual_floor(group: &CloudGroup, board: &Board) -> [f32; 2] {
    let mut floor = [PAD * 2.0, HEADER + PAD];
    for local in &group.panels {
        let Some(panel) = board
            .panels
            .iter()
            .find(|panel| &panel.local_id == local && panel.visible)
        else {
            continue;
        };
        let right = panel.layout.position[0] - group.position[0] + panel.layout.size[0] + PAD;
        let bottom = panel.layout.position[1] - group.position[1] + panel.layout.size[1] + PAD;
        floor[0] = floor[0].max(right);
        floor[1] = floor[1].max(bottom);
    }
    floor
}

fn grid_shape(layout: WorkspaceLayout, count: usize) -> (usize, usize) {
    match layout {
        WorkspaceLayout::Rows => (1, count.max(1)),
        WorkspaceLayout::Columns => (count.max(1), 1),
        WorkspaceLayout::Grid => {
            let cols = layout::ceil_sqrt_usize(count).max(1);
            (cols, count.div_ceil(cols))
        }
    }
}

fn cell_filling(layout: WorkspaceLayout, count: usize, frame: [f32; 2]) -> [f32; 2] {
    let (cols, rows) = grid_shape(layout, count);
    let cols = layout::usize_to_f32(cols);
    let rows = layout::usize_to_f32(rows);
    let gap = layout::TILE_GAP;
    [
        (frame[0] - PAD * 2.0 - (cols - 1.0) * gap) / cols,
        (frame[1] - HEADER - PAD - (rows - 1.0) * gap) / rows,
    ]
}

#[cfg(test)]
mod tests {
    use super::{CloudGroup, CloudGroups, cell_filling, default_frame};
    use crate::{Board, PanelKind, PanelOptions, WorkspaceLayout};

    fn near(actual: [f32; 2], expected: [f32; 2]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(left, right)| (left - right).abs() < 0.05)
    }

    fn cloud_with_members(layout: WorkspaceLayout, count: usize) -> (Board, CloudGroups, Vec<crate::PanelId>) {
        let mut board = Board::new();
        let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(1, "Cloud".into(), local, std::path::PathBuf::new(), [0.0, 0.0]);
        let mut ids = Vec::new();
        for _ in 0..count {
            let id = board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        size: Some(crate::cloud_panel::CHILD_SIZE),
                        ..PanelOptions::default()
                    },
                    workspace,
                )
                .unwrap();
            group.attach(&mut board, id);
            ids.push(id);
        }
        group.set_layout(&mut board, Some(layout));
        let groups = CloudGroups(vec![group]);
        (board, groups, ids)
    }

    #[test]
    fn grid_resize_scales_every_member_and_survives_reconcile() {
        let (mut board, mut groups, ids) = cloud_with_members(WorkspaceLayout::Grid, 2);
        let before = groups.0[0].size;
        let requested = [before[0] + 200.0, before[1] + 80.0];
        assert!(groups.resize_frame(&mut board, 1, requested, [320.0, 220.0]));
        assert!(near(groups.0[0].size, requested));
        let cell = cell_filling(WorkspaceLayout::Grid, 2, requested);
        for id in &ids {
            assert!(near(board.panel(*id).unwrap().layout.size, cell));
            let panel = board.panel(*id).unwrap();
            let (min, max) = groups.0[0].bounds();
            assert!(panel.layout.position[0] >= min[0]);
            assert!(panel.layout.position[1] >= min[1]);
            assert!(panel.layout.position[0] + panel.layout.size[0] <= max[0] + 0.05);
            assert!(panel.layout.position[1] + panel.layout.size[1] <= max[1] + 0.05);
        }
        let size = groups.0[0].size;
        groups.reconcile(&mut board);
        assert!(near(groups.0[0].size, size));
        assert!(near(board.panel(ids[0]).unwrap().layout.size, cell));
    }

    #[test]
    fn shrinking_stops_at_the_member_minimum() {
        let (mut board, mut groups, ids) = cloud_with_members(WorkspaceLayout::Rows, 2);
        let min_member = [320.0, 220.0];
        assert!(groups.resize_frame(&mut board, 1, [10.0, 10.0], min_member));
        for id in &ids {
            assert!(near(board.panel(*id).unwrap().layout.size, min_member));
        }
        let hugged = groups.0[0].size;
        groups.reconcile(&mut board);
        assert!(near(groups.0[0].size, hugged));
        assert!(hugged[0] > 10.0 && hugged[1] > 10.0);
    }

    #[test]
    fn manual_resize_keeps_positions_and_stops_at_the_panels() {
        let (mut board, mut groups, ids) = cloud_with_members(WorkspaceLayout::Grid, 2);
        groups.0[0].set_layout(&mut board, None);
        let positions: Vec<[u32; 2]> = ids
            .iter()
            .map(|id| board.panel(*id).unwrap().layout.position.map(f32::to_bits))
            .collect();
        let sizes: Vec<[u32; 2]> = ids
            .iter()
            .map(|id| board.panel(*id).unwrap().layout.size.map(f32::to_bits))
            .collect();
        let grown = [groups.0[0].size[0] + 240.0, groups.0[0].size[1] + 120.0];
        assert!(groups.resize_frame(&mut board, 1, grown, [320.0, 220.0]));
        assert!(near(groups.0[0].size, grown));
        assert!(groups.resize_frame(&mut board, 1, [40.0, 40.0], [320.0, 220.0]));
        assert!(groups.0[0].size[0] > 40.0 && groups.0[0].size[1] > 40.0);
        let placed: Vec<[u32; 2]> = ids
            .iter()
            .map(|id| board.panel(*id).unwrap().layout.position.map(f32::to_bits))
            .collect();
        let kept: Vec<[u32; 2]> = ids
            .iter()
            .map(|id| board.panel(*id).unwrap().layout.size.map(f32::to_bits))
            .collect();
        assert_eq!(placed, positions);
        assert_eq!(kept, sizes);
    }

    #[test]
    fn empty_resize_keeps_a_larger_frame_and_the_default_floor() {
        let mut board = Board::new();
        let workspace = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(workspace).unwrap().local_id.clone();
        let group = CloudGroup::new(7, "Empty".into(), local, std::path::PathBuf::new(), [12.0, 24.0]);
        let mut groups = CloudGroups(vec![group]);
        assert!(groups.resize_frame(&mut board, 7, [900.0, 820.0], [320.0, 220.0]));
        assert!(near(groups.0[0].size, [900.0, 820.0]));
        groups.reconcile(&mut board);
        assert!(near(groups.0[0].size, [900.0, 820.0]));
        assert!(groups.resize_frame(&mut board, 7, [20.0, 20.0], [320.0, 220.0]));
        assert!(near(groups.0[0].size, default_frame()));
    }

    #[test]
    fn growing_cloud_pushes_the_next_cloud_aside() {
        let (mut board, mut groups, _) = cloud_with_members(WorkspaceLayout::Columns, 1);
        let local = groups.0[0].workspace.clone();
        groups.0.push(CloudGroup::new(
            2,
            "Next".into(),
            local,
            std::path::PathBuf::new(),
            [groups.0[0].overview_bounds().1[0] + 30.0, 0.0],
        ));
        let start = groups.0[1].position[0];
        let requested = [groups.0[0].size[0] + 500.0, groups.0[0].size[1]];
        assert!(groups.resize_frame(&mut board, 1, requested, [320.0, 220.0]));
        assert!(groups.0[1].position[0] > start);
        assert!(groups.0[1].position[0] > groups.0[0].overview_bounds().1[0]);
    }

    #[test]
    fn collapsed_cloud_does_not_resize() {
        let (mut board, mut groups, _) = cloud_with_members(WorkspaceLayout::Grid, 1);
        groups.0[0].set_collapsed(&mut board, true);
        let size = groups.0[0].size;
        assert!(!groups.resize_frame(&mut board, 1, [1400.0, 900.0], [320.0, 220.0]));
        assert_eq!(groups.0[0].size.map(f32::to_bits), size.map(f32::to_bits));
    }

    #[test]
    fn non_finite_size_is_ignored() {
        let (mut board, mut groups, _) = cloud_with_members(WorkspaceLayout::Grid, 1);
        let size = groups.0[0].size;
        assert!(!groups.resize_frame(&mut board, 1, [f32::NAN, 400.0], [320.0, 220.0]));
        assert_eq!(groups.0[0].size.map(f32::to_bits), size.map(f32::to_bits));
    }
}
