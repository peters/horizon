//! Opt-in cloud-panel prototype: grouping of ordinary panels, not a new runtime.
mod capabilities;
mod fixture;

pub use horizon_cloud::Connection as CloudConnection;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Board, PanelId, WorkspaceId, WorkspaceLayout};
pub use fixture::{PrototypeSnapshot, load, prepare_repository, save};
pub use horizon_cloud::{BrowserEngine, Capabilities, CloudConfig, Environment};

pub const HEADER: f32 = 84.0;
pub const PAD: f32 = 14.0;
pub const RUNTIME_WIDTH: f32 = 300.0;
pub const RUNTIME_HEIGHT: f32 = 740.0;
pub const CHILD_SIZE: [f32; 2] = [520.0, 500.0];
pub const CLOUDS: [(u32, &str); 5] = [
    (101, "Web preview"),
    (102, "Pair programming"),
    (103, "Full stack sandbox"),
    (104, "Cloud 4"),
    (105, "Cloud 5"),
];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CloudGroup {
    #[serde(default)]
    pub remote: Option<CloudLaunch>,
    pub issue: u32,
    pub title: String,
    pub workspace: String,
    pub environment: Environment,
    pub cwd: PathBuf,
    pub position: [f32; 2],
    #[serde(default)]
    workspace_position: [f32; 2],
    pub size: [f32; 2],
    pub collapsed: bool,
    #[serde(default)]
    pub layout: Option<WorkspaceLayout>,
    pub panels: Vec<String>,
    /// Only panels hidden by collapse are revealed by expand.
    hidden: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CloudLaunch {
    #[serde(default)]
    pub deployment_started: bool,
    pub id: String,
    pub revision: String,
    pub profile_name: String,
    pub profile: horizon_cloud::Profile,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct CloudGroups(pub Vec<CloudGroup>);

impl CloudGroup {
    #[must_use]
    pub fn new(issue: u32, title: String, workspace: String, cwd: PathBuf, position: [f32; 2]) -> Self {
        Self {
            remote: None,
            issue,
            title,
            workspace,
            cwd,
            position,
            workspace_position: [0.0, 0.0],
            environment: Environment::prototype(format!("issue-{issue}")),
            size: [CHILD_SIZE[0] + PAD * 2.0, CHILD_SIZE[1] + HEADER + PAD],
            collapsed: false,
            layout: None,
            panels: Vec::new(),
            hidden: Vec::new(),
        }
    }

    #[must_use]
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let height = if self.collapsed { HEADER } else { self.size[1] };
        (
            self.position,
            [self.position[0] + self.size[0], self.position[1] + height],
        )
    }

    pub fn attach(&mut self, board: &mut Board, id: PanelId) {
        let Some(panel) = board.panel(id) else { return };
        if board.workspace_id_by_local_id(&self.workspace) != Some(panel.workspace_id)
            || board
                .cloud_groups
                .0
                .iter()
                .any(|group| group.environment.id != self.environment.id && group.panels.contains(&panel.local_id))
        {
            return;
        }
        if board.panel(id).is_some_and(|p| self.panels.contains(&p.local_id)) {
            return;
        }
        self.set_collapsed(board, false);
        let position = board.panel(id).map_or(self.position, |p| p.layout.position);
        if let Some(panel) = board.panel_mut(id) {
            if !self.panels.contains(&panel.local_id) {
                self.panels.push(panel.local_id.clone());
            }
            panel.layout.position = position;
        }
        if let Some(panel) = board.panel(id) {
            for (axis, coordinate) in position.iter().enumerate() {
                self.size[axis] = self.size[axis].max(coordinate - self.position[axis] + panel.layout.size[axis] + PAD);
            }
        }
        self.reconcile(board);
    }

    #[must_use]
    pub fn next_position(&self, board: &Board) -> [f32; 2] {
        let x = board
            .panels
            .iter()
            .filter(|p| self.panels.contains(&p.local_id))
            .map(|p| p.layout.position[0] + p.layout.size[0] + PAD)
            .fold(self.position[0] + PAD, f32::max);
        [x, self.position[1] + HEADER]
    }

    pub fn translate(&mut self, board: &mut Board, delta: [f32; 2]) {
        for (axis, amount) in delta.iter().enumerate() {
            self.position[axis] += amount;
        }
        for panel in &mut board.panels {
            if self.panels.contains(&panel.local_id) {
                for (axis, amount) in delta.iter().enumerate() {
                    panel.layout.position[axis] += amount;
                }
            }
        }
    }

    pub fn set_collapsed(&mut self, board: &mut Board, collapsed: bool) {
        if self.collapsed == collapsed {
            return;
        }
        self.collapsed = collapsed;
        for panel in &mut board.panels {
            if !self.panels.contains(&panel.local_id) {
                continue;
            }
            if collapsed && panel.visible {
                self.hidden.push(panel.local_id.clone());
                panel.visible = false;
                if board.focused == Some(panel.id) {
                    board.focused = None;
                }
            } else if !collapsed && self.hidden.contains(&panel.local_id) {
                panel.visible = true;
            }
        }
        if !collapsed {
            self.hidden.clear();
        }
    }

    /// Adjacent runtime card geometry shared by rendering and input routing.
    #[must_use]
    pub fn runtime_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let min = [self.position[0] + self.size[0] + PAD, self.position[1]];
        (min, [min[0] + RUNTIME_WIDTH, min[1] + RUNTIME_HEIGHT])
    }

    /// Bounds include the adjacent runtime card for overview and collision spacing.
    #[must_use]
    pub fn overview_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let (min, mut max) = self.bounds();
        let (_, runtime_max) = self.runtime_bounds();
        max[0] = runtime_max[0];
        max[1] = max[1].max(runtime_max[1]);
        (min, max)
    }

    pub fn set_layout(&mut self, board: &mut Board, layout: Option<WorkspaceLayout>) {
        self.layout = layout;
        if layout.is_some() {
            self.set_collapsed(board, false);
            self.arrange(board);
        }
    }

    pub fn arrange(&mut self, board: &mut Board) {
        let Some(layout) = self.layout else { return };
        let mut members = self
            .panels
            .iter()
            .filter_map(|id| board.panels.iter().find(|p| &p.local_id == id && p.visible));
        let first = members.next();
        let size = first.map_or(CHILD_SIZE, |p| p.layout.size);
        let count = usize::from(first.is_some()) + members.count();
        let origin = [
            self.position[0] + PAD - crate::layout::WS_INNER_PAD,
            self.position[1] + HEADER - crate::layout::WS_INNER_PAD,
        ];
        self.size = [CHILD_SIZE[0] + PAD * 2.0, CHILD_SIZE[1] + HEADER + PAD];
        let mut index = 0;
        for id in &self.panels {
            let Some(panel) = board.panels.iter_mut().find(|p| &p.local_id == id && p.visible) else {
                continue;
            };
            let (position, size) = crate::board::arranged_panel_layout(origin, layout, index, count, size);
            index += 1;
            panel.move_to(position);
            panel.resize_layout(size);
            for axis in 0..2 {
                self.size[axis] = self.size[axis].max(position[axis] - self.position[axis] + size[axis] + PAD);
            }
        }
    }

    pub fn reconcile(&mut self, board: &mut Board) {
        let workspace = board.workspace_id_by_local_id(&self.workspace);
        if let Some(ws) = workspace.and_then(|id| board.workspace(id)) {
            for axis in 0..2 {
                self.position[axis] += ws.position[axis] - self.workspace_position[axis];
            }
            self.workspace_position = ws.position;
        }
        self.panels.retain(|id| board.panels.iter().any(|p| &p.local_id == id));
        if let Some(workspace) = workspace {
            let moved: Vec<_> = board
                .panels
                .iter()
                .filter(|p| self.panels.contains(&p.local_id) && p.workspace_id != workspace)
                .map(|p| p.id)
                .collect();
            for id in moved {
                board.assign_panel_to_workspace(id, workspace);
            }
        }
        self.hidden.retain(|id| self.panels.contains(id));
        for panel in &mut board.panels {
            if !self.panels.contains(&panel.local_id) {
                continue;
            }
            // Sidebar reveal must expand the group, never leave an invisible focused terminal.
            if self.collapsed && board.focused == Some(panel.id) {
                self.collapsed = false;
            }
        }
        if !self.collapsed {
            for panel in &mut board.panels {
                if self.hidden.contains(&panel.local_id) {
                    panel.visible = true;
                }
                if !self.panels.contains(&panel.local_id) {
                    continue;
                }
                let offsets = [PAD, HEADER];
                for (axis, offset) in offsets.iter().enumerate() {
                    self.size[axis] = self.size[axis].max(offset + panel.layout.size[axis] + PAD);
                    let min = self.position[axis] + offset;
                    // Fractional zoom can round the equal upper edge just below the lower edge.
                    let max = (self.position[axis] + self.size[axis] - panel.layout.size[axis] - PAD).max(min);
                    panel.layout.position[axis] = panel.layout.position[axis].clamp(min, max);
                }
            }
            self.hidden.clear();
            self.arrange(board);
        }
    }
}

impl CloudGroups {
    pub fn reconcile(&mut self, board: &mut Board) {
        for index in 0..self.0.len() {
            let before = self.0[index].size;
            self.0[index].reconcile(board);
            if self.0[index].size.iter().zip(before).any(|(new, old)| *new > old) {
                self.make_room(board, index);
            }
        }
    }

    /// Resize within a cloud without invoking the parent workspace's collision policy.
    pub fn resize_panel(&mut self, board: &mut Board, id: PanelId, size: [f32; 2]) -> bool {
        let Some(panel) = board.panel(id) else { return false };
        let Some(index) = self.0.iter().position(|g| g.panels.contains(&panel.local_id)) else {
            return false;
        };
        let group = &mut self.0[index];
        if group.layout.is_some() {
            for member in &mut board.panels {
                if group.panels.contains(&member.local_id) && member.visible {
                    member.resize_layout(size);
                }
            }
            group.arrange(board);
        } else if let Some(panel) = board.panel_mut(id) {
            panel.resize_layout(size);
            for (axis, extent) in size.iter().enumerate() {
                group.size[axis] =
                    group.size[axis].max(panel.layout.position[axis] - group.position[axis] + extent + PAD);
            }
        }
        group.reconcile(board);
        self.make_room(board, index);
        true
    }

    /// Make room for an expanded frame without changing any panel's membership.
    pub fn make_room(&mut self, board: &mut Board, expanded: usize) {
        let order: Vec<_> = std::iter::once(expanded)
            .chain((0..self.0.len()).filter(|i| *i != expanded))
            .collect();
        for (offset, &index) in order.iter().enumerate().skip(1) {
            for _ in 0..offset {
                for &other in &order[..offset] {
                    if self.0[index].workspace != self.0[other].workspace {
                        continue;
                    }
                    let (min, max) = self.0[index].overview_bounds();
                    let (other_min, other_max) = self.0[other].overview_bounds();
                    if min[0] < other_max[0] && max[0] > other_min[0] && min[1] < other_max[1] && max[1] > other_min[1]
                    {
                        self.0[index].translate(board, [other_max[0] + PAD * 2.0 - min[0], 0.0]);
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn contains_panel(&self, board: &Board, id: PanelId) -> bool {
        board
            .panel(id)
            .is_some_and(|p| self.0.iter().any(|g| g.panels.contains(&p.local_id)))
    }

    #[must_use]
    pub fn at_position(&self, board: &Board, workspace: WorkspaceId, position: [f32; 2]) -> Option<usize> {
        self.0.iter().position(|g| {
            let (min, max) = g.bounds();
            !g.collapsed
                && board.workspace_id_by_local_id(&g.workspace) == Some(workspace)
                && position[0] >= min[0]
                && position[0] <= max[0]
                && position[1] >= min[1]
                && position[1] <= max[1]
        })
    }

    pub fn adopt_intersecting(&mut self, board: &Board) {
        for panel in &board.panels {
            if !panel.visible || self.0.iter().any(|g| g.panels.contains(&panel.local_id)) {
                continue;
            }
            let p = panel.layout.position;
            let size = panel.layout.size;
            let winner = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.remote.is_none()
                        && !g.collapsed
                        && board.workspace_id_by_local_id(&g.workspace) == Some(panel.workspace_id)
                })
                .map(|(index, g)| {
                    let (min, max) = g.bounds();
                    let width = (max[0].min(p[0] + size[0]) - min[0].max(p[0])).max(0.0);
                    let height = (max[1].min(p[1] + size[1]) - (min[1] + HEADER).max(p[1])).max(0.0);
                    (index, width * height)
                })
                .filter(|(_, area)| *area > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index);
            if let Some(index) = winner {
                self.0[index].panels.push(panel.local_id.clone());
            }
        }
    }

    #[must_use]
    pub fn fitted_view(min: [f32; 2], max: [f32; 2], canvas: [f32; 2]) -> crate::CanvasViewState {
        let zoom = crate::clamp_canvas_zoom(
            ((canvas[0] - 70.0) / (max[0] - min[0]).max(1.0)).min((canvas[1] - 140.0) / (max[1] - min[1]).max(1.0)),
        )
        .min(1.0);
        crate::CanvasViewState::new(
            [
                canvas[0] * 0.5 - min[0].midpoint(max[0]) * zoom,
                35.0 + canvas[1] * 0.5 - min[1].midpoint(max[1]) * zoom,
            ],
            zoom,
        )
    }

    /// Fit an unobstructed canvas region using the shared persisted zoom limits.
    #[must_use]
    pub fn fitted_region(min: [f32; 2], max: [f32; 2], origin: [f32; 2], size: [f32; 2]) -> crate::CanvasViewState {
        let zoom = crate::clamp_canvas_zoom(
            ((size[0] - 24.0).max(1.0) / (max[0] - min[0]).max(1.0))
                .min((size[1] - 24.0).max(1.0) / (max[1] - min[1]).max(1.0)),
        )
        .min(1.0);
        crate::CanvasViewState::new(
            [
                origin[0] + size[0] * 0.5 - min[0].midpoint(max[0]) * zoom,
                origin[1] + size[1] * 0.5 - min[1].midpoint(max[1]) * zoom,
            ],
            zoom,
        )
    }

    pub fn restore_visibility(&mut self, board: &mut Board) {
        for group in &mut self.0 {
            if !group.collapsed {
                continue;
            }
            for panel in &mut board.panels {
                if group.panels.contains(&panel.local_id) {
                    panel.visible = false;
                    if board.focused == Some(panel.id) {
                        board.focused = None;
                    }
                }
            }
        }
    }

    pub fn extend_workspace_bounds(&self, board: &Board, bounds: &mut HashMap<WorkspaceId, ([f32; 2], [f32; 2])>) {
        for group in &self.0 {
            let Some(id) = board.workspace_id_by_local_id(&group.workspace) else {
                continue;
            };
            let (min, max) = group.overview_bounds();
            let entry = bounds.entry(id).or_insert((min, max));
            for axis in 0..2 {
                entry.0[axis] = entry.0[axis].min(min[axis]);
                entry.1[axis] = entry.1[axis].max(max[axis]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelKind, PanelOptions};

    #[test]
    fn fractional_panel_resize_keeps_containment_bounds_ordered() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![CloudGroup::new(
            1,
            "a".into(),
            local,
            PathBuf::new(),
            [24.0, 798.0],
        )]);
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    size: Some(CHILD_SIZE),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        groups.0[0].attach(&mut board, id);
        groups.0[0].set_layout(&mut board, Some(WorkspaceLayout::Grid));
        for delta in 0..100_u16 {
            assert!(groups.resize_panel(
                &mut board,
                id,
                [520.0 + f32::from(delta) * 0.1, 500.0 + f32::from(delta) * 0.1]
            ));
            groups.reconcile(&mut board);
            assert!(board.panel(id).unwrap().layout.position.iter().all(|p| p.is_finite()));
        }
    }

    #[test]
    fn resizing_later_child_resizes_grid_and_moves_neighbor_with_its_runtime() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![
            CloudGroup::new(1, "a".into(), local.clone(), PathBuf::new(), [0.0, 0.0]),
            CloudGroup::new(2, "b".into(), local, PathBuf::new(), [1450.0, 0.0]),
        ]);
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        size: Some(CHILD_SIZE),
                        ..PanelOptions::default()
                    },
                    ws,
                )
                .unwrap();
            groups.0[0].attach(&mut board, id);
            ids.push(id);
        }
        groups.0[0].set_layout(&mut board, Some(WorkspaceLayout::Grid));
        assert!(groups.resize_panel(&mut board, ids[1], [800.0, 600.0]));
        groups.reconcile(&mut board);
        for id in &ids {
            assert!(
                board
                    .panel(*id)
                    .unwrap()
                    .layout
                    .size
                    .into_iter()
                    .zip([800.0, 600.0])
                    .all(|(a, b)| (a - b).abs() < f32::EPSILON)
            );
        }
        assert!(groups.0[1].position[0] > groups.0[0].overview_bounds().1[0]);
        board.panel_mut(ids[0]).unwrap().resize_layout([1000.0, 600.0]);
        groups.reconcile(&mut board);
        assert!(groups.0[1].position[0] > groups.0[0].overview_bounds().1[0]);
        board.close_panel(ids[1]);
        groups.reconcile(&mut board);
        assert_eq!(groups.0[0].panels.len(), 1);
        assert!(groups.0[1].panels.is_empty());
    }

    #[test]
    fn independent_layouts_reflow_members_and_survive_serialization() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut a = CloudGroup::new(1, "a".into(), local.clone(), PathBuf::new(), [0.0, 0.0]);
        let mut b = CloudGroup::new(2, "b".into(), local, PathBuf::new(), [1800.0, 0.0]);
        for group in [&mut a, &mut b] {
            for _ in 0..3 {
                let id = board
                    .create_panel(
                        PanelOptions {
                            kind: PanelKind::Usage,
                            size: Some(CHILD_SIZE),
                            ..PanelOptions::default()
                        },
                        ws,
                    )
                    .unwrap();
                group.attach(&mut board, id);
            }
        }
        a.set_layout(&mut board, Some(WorkspaceLayout::Rows));
        b.set_layout(&mut board, Some(WorkspaceLayout::Grid));
        let positions = |group: &CloudGroup, board: &Board| -> Vec<[f32; 2]> {
            group
                .panels
                .iter()
                .map(|id| board.panels.iter().find(|p| &p.local_id == id).unwrap().layout.position)
                .collect()
        };
        let before_b = positions(&b, &board);
        let rows = positions(&a, &board);
        assert!((rows[0][0] - rows[1][0]).abs() < f32::EPSILON);
        assert!(rows[0][1] < rows[1][1]);
        assert!((before_b[0][1] - before_b[1][1]).abs() < f32::EPSILON);
        assert!(before_b[2][1] > before_b[0][1]);
        a.set_layout(&mut board, Some(WorkspaceLayout::Columns));
        assert_eq!(positions(&b, &board), before_b);
        let columns = positions(&a, &board);
        assert!((columns[0][1] - columns[1][1]).abs() < f32::EPSILON);
        assert!(columns[0][0] < columns[1][0]);
        let encoded = serde_json::to_string(&CloudGroups(vec![a, b])).unwrap();
        let restored: CloudGroups = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.0[0].layout, Some(WorkspaceLayout::Columns));
        assert_eq!(restored.0[1].layout, Some(WorkspaceLayout::Grid));
        assert_eq!(restored.0[0].panels.len(), 3);
        assert_eq!(restored.0[1].panels.len(), 3);
    }

    #[test]
    fn collapse_move_expand_preserves_panel_identity_and_prior_visibility() {
        let mut board = Board::new();
        let ws = board.create_workspace("test");
        let mut group = CloudGroup::new(
            1,
            "issue".into(),
            board.workspace(ws).unwrap().local_id.clone(),
            PathBuf::new(),
            [0.0, 0.0],
        );
        let a = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        let b = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        group.attach(&mut board, a);
        group.attach(&mut board, b);
        board.panel_mut(b).unwrap().visible = false;
        let before = board.panel(a).unwrap().layout.position;
        group.set_collapsed(&mut board, true);
        assert!(!board.panel(a).unwrap().visible);
        group.translate(&mut board, [10.0, 20.0]);
        group.set_collapsed(&mut board, false);
        assert!(board.panel(a).unwrap().visible);
        assert!(!board.panel(b).unwrap().visible);
        let after = board.panel(a).unwrap().layout.position;
        assert!((after[0] - before[0] - 10.0).abs() < f32::EPSILON);
        assert!((after[1] - before[1] - 20.0).abs() < f32::EPSILON);
        assert_eq!(board.panels.len(), 2);
        board.close_panel(a);
        group.reconcile(&mut board);
        assert_eq!(group.panels.len(), 1);
    }
    #[test]
    fn binding_survives_drag_into_another_cloud_and_workspace() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("test", [0.0, 0.0]);
        let other = board.create_workspace("other");
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![
            CloudGroup::new(1, "a".into(), local.clone(), PathBuf::from("a"), [0.0, 0.0]),
            CloudGroup::new(2, "b".into(), local, PathBuf::from("b"), [1000.0, 0.0]),
        ]);
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    position: Some([20.0, HEADER]),
                    size: Some(CHILD_SIZE),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        groups.adopt_intersecting(&board);
        assert_eq!(groups.0[0].panels.len(), 1);
        board.panel_mut(id).unwrap().layout.position = [1100.0, HEADER];
        groups.adopt_intersecting(&board);
        for group in &mut groups.0 {
            group.reconcile(&mut board);
        }
        assert!(groups.0[1].panels.is_empty());
        assert!(board.panel(id).unwrap().layout.position[0] < 1000.0);
        board.assign_panel_to_workspace(id, other);
        groups.0[0].reconcile(&mut board);
        assert_eq!(board.panel(id).unwrap().workspace_id, ws);
    }

    #[test]
    fn empty_cloud_keeps_its_workspace_after_restore_and_cannot_move_members() {
        let mut board = Board::new();
        let ws = board.create_workspace("cloud workspace");
        let other = board.create_workspace("other workspace");
        let local = board.workspace(ws).unwrap().local_id.clone();
        board.cloud_groups.0.push(CloudGroup::new(
            1,
            "Cloud".into(),
            local.clone(),
            PathBuf::new(),
            [0.0, 0.0],
        ));
        let snapshot = crate::RuntimeState::from_board(
            &board,
            crate::WindowConfig::default(),
            crate::CanvasViewState::default(),
        );
        let mut restored = Board::from_runtime_state(&snapshot).unwrap();
        restored.remove_empty_workspaces();
        let restored_ws = restored.workspace_id_by_local_id(&local).unwrap();
        restored.remove_workspace(restored_ws);
        assert!(restored.workspace(restored_ws).is_some());
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        let mut group = board.cloud_groups.0[0].clone();
        group.attach(&mut board, id);
        board.cloud_groups.0[0] = group;
        board.assign_panel_to_workspace(id, other);
        assert_eq!(board.panel_workspace_id(id), Some(ws));
        let mut second = CloudGroup::new(2, "Other cloud".into(), local, PathBuf::new(), [800.0, 0.0]);
        second.attach(&mut board, id);
        assert!(second.panels.is_empty());
    }

    #[test]
    fn collapsed_groups_restore_hidden_and_last_child_keeps_workspace() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("test", [0.0, 0.0]);
        let mut groups = CloudGroups::default();
        for issue in [1, 2] {
            let id = board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        ..PanelOptions::default()
                    },
                    ws,
                )
                .unwrap();
            let mut group = CloudGroup::new(
                issue,
                "issue".into(),
                board.workspace(ws).unwrap().local_id.clone(),
                PathBuf::new(),
                [0.0, 0.0],
            );
            group.attach(&mut board, id);
            group.set_collapsed(&mut board, true);
            groups.0.push(group);
        }
        let snapshot = crate::RuntimeState::from_board(
            &board,
            crate::WindowConfig::default(),
            crate::CanvasViewState::default(),
        );
        let mut restored = Board::from_runtime_state(&snapshot).unwrap();
        groups.restore_visibility(&mut restored);
        for group in &mut groups.0 {
            group.reconcile(&mut restored);
        }
        assert!(restored.panels.iter().all(|p| !p.visible));
        assert!(groups.0.iter().all(|g| g.collapsed));
        restored.retain_workspace_when_empty(ws);
        let ids: Vec<_> = restored.panels.iter().map(|p| p.id).collect();
        for id in ids {
            restored.close_panel(id);
        }
        assert!(restored.workspace(ws).is_some());
    }
}
