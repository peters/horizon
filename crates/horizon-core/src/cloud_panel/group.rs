//! Geometry, membership and reconciliation of one cloud group.
use super::{
    CHILD_SIZE, CONTENT_GAP, CloudGeometry, CloudGroup, Environment, HEADER, LEGACY_TOOLBAR_CONTROLS_HEIGHT,
    LEGACY_TOOLBAR_HEIGHT, PAD, RUNTIME_HEIGHT, RUNTIME_WIDTH, STATUS_HEIGHT, resize,
};
use crate::{Board, PanelId, WorkspaceLayout};
use std::path::PathBuf;

impl CloudGroup {
    #[must_use]
    pub fn new(issue: u32, title: String, workspace: String, cwd: PathBuf, position: [f32; 2]) -> Self {
        Self {
            remote: None,
            siblings: Vec::new(),
            issue,
            title,
            workspace,
            cwd,
            position,
            workspace_position: [0.0, 0.0],
            environment: Environment::prototype(format!("issue-{issue}")),
            size: [CHILD_SIZE[0] + PAD * 2.0, CHILD_SIZE[1] + HEADER + PAD],
            collapsed: false,
            legacy_toolbar: None,
            layout: Some(WorkspaceLayout::default()),
            panels: Vec::new(),
            hidden: Vec::new(),
        }
    }

    #[must_use]
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let height = if self.collapsed {
            self.header_height()
        } else {
            self.size[1]
        };
        (
            self.position,
            [self.position[0] + self.size[0], self.position[1] + height],
        )
    }

    /// Space reserved for identity, and a production cloud's status strip, before session panels.
    #[must_use]
    pub fn header_height(&self) -> f32 {
        self.header_chrome_height() + if self.remote.is_some() { CONTENT_GAP } else { 0.0 }
    }

    /// The painted header: `header_height` without the gap that keeps panels off its edge.
    #[must_use]
    pub fn header_chrome_height(&self) -> f32 {
        HEADER + if self.remote.is_some() { STATUS_HEIGHT } else { 0.0 }
    }

    pub(super) fn minimum_width(&self) -> f32 {
        if self.remote.is_some() {
            CHILD_SIZE[0] + PAD * 2.0
        } else {
            PAD * 2.0
        }
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
        self.set_collapsed_state(board, false);
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
        [x, self.position[1] + self.header_height()]
    }

    /// Move the cloud by the same delta as its workspace, including the
    /// remembered workspace origin so a later reconcile does not apply it twice.
    pub(crate) fn translate_with_workspace(&mut self, delta: [f32; 2]) {
        for (axis, amount) in delta.iter().enumerate() {
            self.position[axis] += amount;
            self.workspace_position[axis] += amount;
        }
    }

    pub fn translate(&mut self, board: &mut Board, delta: [f32; 2]) {
        self.translate_with_collisions(board, delta, true);
    }

    pub(super) fn translate_with_collisions(&mut self, board: &mut Board, delta: [f32; 2], resolve_collisions: bool) {
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
        self.publish(board, resolve_collisions);
    }

    pub fn set_collapsed(&mut self, board: &mut Board, collapsed: bool) {
        if self.collapsed != collapsed {
            self.set_collapsed_state(board, collapsed);
            self.publish(board, true);
        }
    }

    fn set_collapsed_state(&mut self, board: &mut Board, collapsed: bool) {
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

    /// Runtime status or prototype card geometry shared by rendering and input routing.
    /// A production cloud without panels shows its steps and output in the body, so
    /// the body belongs to the runtime until the first panel arrives. Otherwise its
    /// status is part of the header, which pans like the rest of it, and the bounds are
    /// an empty band along the header's bottom edge.
    #[must_use]
    pub fn runtime_bounds(&self) -> ([f32; 2], [f32; 2]) {
        if self.remote.is_some() {
            let top = self.position[1] + self.header_height();
            let left_right = [self.position[0], self.position[0] + self.size[0]];
            if self.panels.is_empty() && !self.collapsed {
                return ([left_right[0], top], [left_right[1], self.position[1] + self.size[1]]);
            }
            return ([left_right[0], top], [left_right[1], top]);
        }
        let min = [self.position[0] + self.size[0] + PAD, self.position[1]];
        (min, [min[0] + RUNTIME_WIDTH, min[1] + RUNTIME_HEIGHT])
    }

    /// Bounds include runtime controls for overview and collision spacing.
    #[must_use]
    pub fn overview_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let (min, mut max) = self.bounds();
        let (_, runtime_max) = self.runtime_bounds();
        max[0] = max[0].max(runtime_max[0]);
        max[1] = max[1].max(runtime_max[1]);
        (min, max)
    }

    /// Overview bounds where the cloud is drawn. A cloud follows its
    /// workspace's moves on its next reconcile, so a move it has not been
    /// reconciled with yet still applies.
    #[must_use]
    pub fn placed_overview_bounds(&self, board: &Board) -> ([f32; 2], [f32; 2]) {
        let (min, max) = self.overview_bounds();
        let shift = board
            .workspace_id_by_local_id(&self.workspace)
            .and_then(|id| board.workspace(id))
            .map_or([0.0, 0.0], |workspace| {
                [
                    workspace.position[0] - self.workspace_position[0],
                    workspace.position[1] - self.workspace_position[1],
                ]
            });
        (
            [min[0] + shift[0], min[1] + shift[1]],
            [max[0] + shift[0], max[1] + shift[1]],
        )
    }

    pub fn set_layout(&mut self, board: &mut Board, layout: Option<WorkspaceLayout>) {
        self.layout = layout;
        if layout.is_some() {
            self.set_collapsed_state(board, false);
            self.arrange(board);
        }
        self.publish(board, true);
    }

    pub fn arrange(&mut self, board: &mut Board) {
        let Some(layout) = self.layout else { return };
        let mut members = self
            .panels
            .iter()
            .filter_map(|id| board.panels.iter().find(|p| &p.local_id == id && p.visible));
        let first = members.next();
        let count = usize::from(first.is_some()) + members.count();
        let origin = [
            self.position[0] + PAD - crate::layout::WS_INNER_PAD,
            self.position[1] + self.header_height() - crate::layout::WS_INNER_PAD,
        ];
        // An empty cloud keeps at least the default frame, including a larger
        // size chosen from the corner. Occupied presets start from chrome and
        // grow to their cells, so that same corner can shrink the frame.
        if count == 0 {
            let mut floor = resize::default_frame();
            floor[1] += self.header_height() - HEADER;
            self.size = [self.size[0].max(floor[0]), self.size[1].max(floor[1])];
            return;
        }
        let size = first.map_or(CHILD_SIZE, |panel| panel.layout.size);
        self.size = [self.minimum_width(), self.header_height() + PAD];
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

    pub fn reconcile(&mut self, board: &mut Board) -> bool {
        self.reconcile_with_collisions(board, true)
    }

    pub(super) fn reconcile_with_collisions(&mut self, board: &mut Board, resolve_collisions: bool) -> bool {
        // Callers such as attach and resize mutate this group before reconcile.
        // Compare with the board's geometry from before those mutations,
        // without cloning the remote payload on the per-frame path.
        let stored = board
            .cloud_groups
            .0
            .iter()
            .find(|group| group.environment.id == self.environment.id)
            .map(|group| CloudGeometry {
                position: group.position,
                size: group.size,
                workspace_position: group.workspace_position,
                collapsed: group.collapsed,
                panels_matched: group.panels == self.panels,
                panel_count: self.panels.len(),
            });
        self.size[0] = self.size[0].max(self.minimum_width());
        let workspace = board.workspace_id_by_local_id(&self.workspace);
        if let Some(ws) = workspace.and_then(|id| board.workspace_mut(id)) {
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
                board.reconcile_panel_workspace(id, workspace);
            }
        }
        self.hidden.retain(|id| self.panels.contains(id));
        self.release_legacy_toolbar_space(board);
        self.reserve_header_space(board);
        let header = self.header_height();
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
            if self.remote.is_some() && self.panels.is_empty() {
                self.size[1] = self.size[1].max(resize::default_frame()[1] + self.header_height() - HEADER);
            }
            for panel in &mut board.panels {
                if self.hidden.contains(&panel.local_id) {
                    panel.visible = true;
                }
                if !self.panels.contains(&panel.local_id) {
                    continue;
                }
                let offsets = [PAD, header];
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
        let changed = stored.as_ref().is_none_or(|previous| previous.differs_from(self));
        if changed {
            // Direct callers (attach, resize) ignore the returned flag, so
            // publish here. Unchanged reconciles, including the per-frame
            // pass, do not reapply the workspace preset.
            self.publish(board, resolve_collisions);
        }
        changed
    }

    /// Sessions saved under the earlier summary card sit that card's height lower;
    /// move them, hidden ones included, up under the status strip once.
    fn release_legacy_toolbar_space(&mut self, board: &mut Board) {
        let Some(expanded) = self.legacy_toolbar.take() else {
            return;
        };
        if self.remote.is_none() {
            return;
        }
        let legacy_top =
            HEADER + LEGACY_TOOLBAR_HEIGHT + PAD + if expanded { LEGACY_TOOLBAR_CONTROLS_HEIGHT } else { 0.0 };
        let content_top = self.position[1] + self.header_height();
        let first_top = board
            .panels
            .iter()
            .filter(|panel| self.panels.contains(&panel.local_id))
            .map(|panel| panel.layout.position[1])
            .reduce(f32::min);
        // Only the reserved band is released; sessions a person placed lower keep their gap.
        let shift = first_top.map_or(legacy_top - self.header_height(), |top| {
            (top - content_top).min(legacy_top - self.header_height())
        });
        if shift <= 0.0 {
            return;
        }
        for panel in &mut board.panels {
            if self.panels.contains(&panel.local_id) {
                panel.layout.position[1] -= shift;
            }
        }
        self.size[1] -= shift;
    }

    fn reserve_header_space(&mut self, board: &mut Board) {
        if self.remote.is_none() {
            return;
        }
        let content_top = self.position[1] + self.header_height();
        let first_top = board
            .panels
            .iter()
            .filter(|panel| self.panels.contains(&panel.local_id))
            .map(|panel| panel.layout.position[1])
            .fold(content_top, f32::min);
        let shift = content_top - first_top;
        if shift > 0.0 {
            // Older saved manual layouts must move together, including hidden members.
            for panel in &mut board.panels {
                if self.panels.contains(&panel.local_id) {
                    panel.layout.position[1] += shift;
                }
            }
            self.size[1] += shift;
        }
    }

    /// Copy this group's geometry onto the board and reapply the workspace
    /// preset so arranged panels stay clear of the cloud.
    fn publish(&self, board: &mut Board, resolve_collisions: bool) {
        if !board
            .cloud_groups
            .0
            .iter()
            .any(|group| group.environment.id == self.environment.id)
        {
            return;
        }
        let workspace_id = board.workspace_id_by_local_id(&self.workspace);
        let before = workspace_id.and_then(|id| board.workspace_frame_rect(id));
        self.write_geometry(board);
        if let Some(workspace_id) = workspace_id {
            if resolve_collisions {
                board.reapply_workspace_layout_after(workspace_id, before);
            } else {
                board.reapply_workspace_layout_if_set(workspace_id);
            }
        }
    }

    fn write_geometry(&self, board: &mut Board) {
        if let Some(slot) = board
            .cloud_groups
            .0
            .iter_mut()
            .find(|group| group.environment.id == self.environment.id)
        {
            *slot = self.clone();
        }
    }
}
