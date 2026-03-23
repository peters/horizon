use crate::layout::{
    TILE_GAP, WORKSPACE_GAP, WS_COLLISION_GAP, WS_EMPTY_FRAME_SIZE, WS_FRAME_PAD, WS_FRAME_TOP_EXTRA, WS_INNER_PAD,
    ceil_sqrt_usize, tiled_panel_position, usize_to_f32, workspace_slot_width,
};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelId};
use crate::workspace::{Workspace, WorkspaceId};

use super::{Board, WorkspaceLayout, vec2_eq};

impl Board {
    /// After a panel is resized, push every overlapping sibling panel
    /// within the same workspace along the dominant resize-growth axis,
    /// cascading until nothing overlaps.
    fn resolve_panel_collisions(&mut self, source: PanelId, workspace_id: WorkspaceId, resize_delta: [f32; 2]) {
        let Some(sibling_ids) = self.workspace(workspace_id).map(|ws| ws.panels.clone()) else {
            return;
        };

        let mut queue = vec![source];
        let mut settled = vec![source];

        while let Some(check_id) = queue.pop() {
            let Some(check_panel) = self.panel(check_id) else {
                continue;
            };
            let cp = check_panel.layout.position;
            let cs = check_panel.layout.size;
            let check_rect = [cp[0], cp[1], cp[0] + cs[0], cp[1] + cs[1]];

            let candidates: Vec<PanelId> = sibling_ids.iter().copied().filter(|id| !settled.contains(id)).collect();

            for other_id in candidates {
                let Some(other_panel) = self.panel(other_id) else {
                    continue;
                };
                let op = other_panel.layout.position;
                let os = other_panel.layout.size;
                let other_rect = [op[0], op[1], op[0] + os[0], op[1] + os[1]];

                let push = resize_collision_push(check_rect, other_rect, resize_delta, TILE_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    if let Some(panel) = self.panel_mut(other_id) {
                        panel.move_to([op[0] + push[0], op[1] + push[1]]);
                    }
                    settled.push(other_id);
                    queue.push(other_id);
                }
            }
        }
    }

    /// After the workspace `source` was moved, push every overlapping
    /// workspace along `drag_dir`, cascading until nothing overlaps.
    pub(super) fn resolve_workspace_collisions(&mut self, source: WorkspaceId, drag_dir: [f32; 2]) {
        let workspace_ids: Vec<_> = self.workspaces.iter().map(|workspace| workspace.id).collect();
        self.resolve_workspace_collisions_with_push(source, drag_dir, &workspace_ids, collision_push);
    }

    /// Resolve collisions for `source` against an explicit workspace scope.
    pub(super) fn resolve_workspace_collisions_in_scope(
        &mut self,
        source: WorkspaceId,
        drag_dir: [f32; 2],
        workspace_ids: &[WorkspaceId],
    ) {
        self.resolve_workspace_collisions_with_push(source, drag_dir, workspace_ids, collision_push);
    }

    fn resolve_workspace_resize_collisions_in_scope(
        &mut self,
        source: WorkspaceId,
        resize_delta: [f32; 2],
        workspace_ids: &[WorkspaceId],
    ) {
        self.resolve_workspace_collisions_with_push(source, resize_delta, workspace_ids, resize_collision_push);
    }

    fn resolve_workspace_collisions_with_push(
        &mut self,
        source: WorkspaceId,
        delta: [f32; 2],
        workspace_ids: &[WorkspaceId],
        push_fn: RectCollisionPush,
    ) {
        let mut queue = vec![source];
        let mut settled = vec![source];

        while let Some(check_id) = queue.pop() {
            let Some(check_rect) = self.workspace_frame_rect(check_id) else {
                continue;
            };

            let candidates: Vec<WorkspaceId> = workspace_ids
                .iter()
                .copied()
                .filter(|id| !settled.contains(id))
                .collect();

            for other_id in candidates {
                let Some(other_rect) = self.workspace_frame_rect(other_id) else {
                    continue;
                };
                let push = push_fn(check_rect, other_rect, delta, WS_COLLISION_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    self.translate_workspace(other_id, push);
                    settled.push(other_id);
                    queue.push(other_id);
                }
            }
        }
    }

    pub fn move_panel(&mut self, id: PanelId, position: [f32; 2]) -> bool {
        if self
            .panel(id)
            .is_some_and(|panel| vec2_eq(panel.layout.position, position))
        {
            return false;
        }
        if let Some(workspace_id) = self.panel_workspace_id(id) {
            self.set_workspace_layout(workspace_id, None);
        }
        if let Some(panel) = self.panel_mut(id) {
            panel.move_to(position);
            return true;
        }

        false
    }

    pub fn resize_panel(&mut self, id: PanelId, size: [f32; 2]) -> bool {
        let workspace_ids: Vec<_> = self.workspaces.iter().map(|workspace| workspace.id).collect();
        self.resize_panel_with_workspace_scope(id, size, &workspace_ids)
    }

    pub fn resize_panel_with_workspace_scope(
        &mut self,
        id: PanelId,
        size: [f32; 2],
        workspace_collision_ids: &[WorkspaceId],
    ) -> bool {
        if self.panel(id).is_some_and(|panel| vec2_eq(panel.layout.size, size)) {
            return false;
        }
        let ws_id = self.panel_workspace_id(id);
        let old_size = self.panel(id).map(|panel| panel.layout.size);
        if let Some(workspace_id) = ws_id
            && let Some(layout) = self.workspace_layout_value(workspace_id)
        {
            self.apply_workspace_layout_with_panel_size(workspace_id, layout, size);
            if let Some(old) = old_size {
                let delta = [size[0] - old[0], size[1] - old[1]];
                if resize_expands(delta) {
                    self.resolve_workspace_resize_collisions_in_scope(workspace_id, delta, workspace_collision_ids);
                }
            }
            return true;
        }

        if let Some(workspace_id) = ws_id {
            self.set_workspace_layout(workspace_id, None);
        }
        if let Some(panel) = self.panel_mut(id) {
            panel.resize_layout(size);
        } else {
            return false;
        }
        if let Some(ws_id) = ws_id {
            let delta = match old_size {
                Some(old) => [size[0] - old[0], size[1] - old[1]],
                None => size,
            };
            if resize_expands(delta) {
                self.resolve_panel_collisions(id, ws_id, delta);
                self.resolve_workspace_resize_collisions_in_scope(ws_id, delta, workspace_collision_ids);
            }
        }
        true
    }

    /// Arrange all panels in a workspace according to a predefined layout.
    /// Panels are equally sized and positioned with gaps.
    pub fn arrange_workspace(&mut self, id: WorkspaceId, layout: WorkspaceLayout) {
        self.apply_workspace_layout(id, layout);
    }

    pub fn clear_workspace_layout(&mut self, id: WorkspaceId) -> bool {
        if self.workspace_layout_value(id).is_none() {
            return false;
        }

        self.set_workspace_layout(id, None);
        true
    }

    /// Align the selected workspaces side by side in a horizontal row,
    /// sorted by their current x position, with consistent vertical
    /// alignment and [`WORKSPACE_GAP`] spacing between frames. Returns the
    /// leftmost workspace ID after alignment, or `None` when fewer than two
    /// selected workspaces exist.
    pub fn align_workspaces_horizontally(&mut self, workspace_ids: &[WorkspaceId]) -> Option<WorkspaceId> {
        if workspace_ids.len() < 2 {
            return None;
        }

        let bounds_map = self.workspace_bounds_map();
        let mut entries: Vec<(WorkspaceId, [f32; 4])> = workspace_ids
            .iter()
            .filter_map(|workspace_id| {
                let ws = self.workspace(*workspace_id)?;
                let rect = if let Some((min, max)) = bounds_map.get(&ws.id) {
                    [
                        min[0] - WS_FRAME_PAD,
                        min[1] - WS_FRAME_PAD - WS_FRAME_TOP_EXTRA,
                        max[0] + WS_FRAME_PAD,
                        max[1] + WS_FRAME_PAD,
                    ]
                } else {
                    let p = ws.position;
                    [p[0], p[1], p[0] + WS_EMPTY_FRAME_SIZE[0], p[1] + WS_EMPTY_FRAME_SIZE[1]]
                };
                Some((ws.id, rect))
            })
            .collect();

        if entries.len() < 2 {
            return None;
        }

        entries.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));

        let leftmost_id = entries[0].0;
        let anchor_y = entries[0].1[1];
        let mut cursor_x = entries[0].1[0];

        for (ws_id, frame) in &entries {
            let frame_width = frame[2] - frame[0];
            self.translate_workspace(*ws_id, [cursor_x - frame[0], anchor_y - frame[1]]);
            cursor_x += frame_width + WORKSPACE_GAP;
        }

        Some(leftmost_id)
    }

    /// Compute the canvas position for the next workspace so it doesn't
    /// overlap with existing ones. Uses fixed-width slots so workspaces
    /// never collide even when fully populated (3 columns).
    pub(super) fn next_workspace_position(&self) -> [f32; 2] {
        let mut right_edge: f32 = 0.0;
        for ws in &self.workspaces {
            right_edge = right_edge.max(ws.position[0] + workspace_slot_width());
        }
        [right_edge, 40.0]
    }

    pub(super) fn default_panel_position(&self, workspace: WorkspaceId) -> [f32; 2] {
        if let Some(ws) = self.workspace(workspace) {
            return self.first_free_tile_position(ws);
        }
        tiled_panel_position([0.0, 0.0], 0)
    }

    pub(super) fn workspace_layout_value(&self, id: WorkspaceId) -> Option<WorkspaceLayout> {
        self.workspace(id).and_then(|workspace| workspace.layout)
    }

    pub(super) fn set_workspace_layout(&mut self, id: WorkspaceId, layout: Option<WorkspaceLayout>) {
        if let Some(workspace) = self.workspace_mut(id) {
            workspace.layout = layout;
        }
    }

    pub(super) fn reflow_workspace_layout(&mut self, id: WorkspaceId) {
        if let Some(layout) = self.workspace_layout_value(id) {
            self.apply_workspace_layout(id, layout);
        }
    }

    pub(super) fn resolve_workspace_collisions_after_frame_growth(
        &mut self,
        id: WorkspaceId,
        previous_frame: Option<[f32; 4]>,
    ) {
        let Some(before) = previous_frame else {
            return;
        };
        let Some(after) = self.workspace_frame_rect(id) else {
            return;
        };

        if after[0] < before[0] - f32::EPSILON {
            self.resolve_workspace_collisions(id, [-1.0, 0.0]);
        }
        if after[1] < before[1] - f32::EPSILON {
            self.resolve_workspace_collisions(id, [0.0, -1.0]);
        }
        if after[2] > before[2] + f32::EPSILON {
            self.resolve_workspace_collisions(id, [1.0, 0.0]);
        }
        if after[3] > before[3] + f32::EPSILON {
            self.resolve_workspace_collisions(id, [0.0, 1.0]);
        }
    }

    pub(super) fn apply_workspace_layout(&mut self, id: WorkspaceId, layout: WorkspaceLayout) {
        let Some(count) = self.workspace(id).map(|workspace| workspace.panels.len()) else {
            return;
        };
        if count == 0 {
            self.set_workspace_layout(id, Some(layout));
            return;
        }

        let current_layout = self.workspace_layout_value(id);
        let panel_size = if current_layout == Some(layout) {
            self.workspace_layout_panel_size(id)
                .or_else(|| layout_panel_size_from_content(layout, count, self.workspace_content_size(id)))
                .unwrap_or(DEFAULT_PANEL_SIZE)
        } else {
            layout_panel_size_from_content(layout, count, self.workspace_content_size(id))
                .or_else(|| self.workspace_layout_panel_size(id))
                .unwrap_or(DEFAULT_PANEL_SIZE)
        };

        self.apply_workspace_layout_with_panel_size(id, layout, panel_size);
    }

    /// Compute the content area of a workspace from its current panel layout
    /// bounds (excluding chrome decoration). Returns `[width, height]` of the
    /// region from the inner-padding edge to the farthest panel edge.
    fn workspace_content_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let workspace = self.workspace(id)?;
        let origin = workspace.position;
        let mut max = [f32::MIN, f32::MIN];
        let mut any = false;
        for panel_id in &workspace.panels {
            if let Some(panel) = self.panel(*panel_id) {
                any = true;
                max[0] = max[0].max(panel.layout.position[0] + panel.layout.size[0]);
                max[1] = max[1].max(panel.layout.position[1] + panel.layout.size[1]);
            }
        }
        if !any {
            return None;
        }
        let min_x = origin[0] + WS_INNER_PAD;
        let min_y = origin[1] + WS_INNER_PAD;
        Some([(max[0] - min_x).max(0.0), (max[1] - min_y).max(0.0)])
    }

    fn workspace_layout_panel_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let workspace = self.workspace(id)?;
        workspace
            .panels
            .iter()
            .find_map(|panel_id| self.panel(*panel_id).map(|panel| panel.layout.size))
    }

    fn apply_workspace_layout_with_panel_size(
        &mut self,
        id: WorkspaceId,
        layout: WorkspaceLayout,
        panel_size: [f32; 2],
    ) {
        let Some((panel_ids, origin)) = self
            .workspace(id)
            .map(|workspace| (workspace.panels.clone(), workspace.position))
        else {
            return;
        };
        let count = panel_ids.len();
        if count == 0 {
            self.set_workspace_layout(id, Some(layout));
            return;
        }

        self.set_workspace_layout(id, Some(layout));

        for (index, panel_id) in panel_ids.iter().enumerate() {
            let (position, size) = arranged_panel_layout(origin, layout, index, count, panel_size);

            if let Some(panel) = self.panel_mut(*panel_id) {
                panel.move_to(position);
                panel.resize_layout(size);
            }
        }
    }

    fn first_free_tile_position(&self, workspace: &Workspace) -> [f32; 2] {
        let occupied: Vec<[f32; 2]> = workspace
            .panels
            .iter()
            .filter_map(|id| self.panel(*id))
            .map(|p| p.layout.position)
            .collect();

        let origin = workspace.position;
        let search_limit = occupied.len();
        for index in 0..=search_limit {
            let candidate = tiled_panel_position(origin, index);
            if !position_occupied(&occupied, candidate) {
                return candidate;
            }
        }

        tiled_panel_position(origin, search_limit)
    }

    /// Returns the visual frame rect `[min_x, min_y, max_x, max_y]` for a
    /// workspace, including the title area and background padding.
    pub(super) fn workspace_frame_rect(&self, id: WorkspaceId) -> Option<[f32; 4]> {
        let workspace = self.workspace(id)?;
        if let Some((min, max)) = self.workspace_bounds(id) {
            Some([
                min[0] - WS_FRAME_PAD,
                min[1] - WS_FRAME_PAD - WS_FRAME_TOP_EXTRA,
                max[0] + WS_FRAME_PAD,
                max[1] + WS_FRAME_PAD,
            ])
        } else {
            let p = workspace.position;
            Some([p[0], p[1], p[0] + WS_EMPTY_FRAME_SIZE[0], p[1] + WS_EMPTY_FRAME_SIZE[1]])
        }
    }
}

fn arranged_panel_layout(
    origin: [f32; 2],
    layout: WorkspaceLayout,
    index: usize,
    count: usize,
    panel_size: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    match layout {
        WorkspaceLayout::Rows => {
            let x = origin[0] + WS_INNER_PAD;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
        WorkspaceLayout::Columns => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD;
            ([x, y], panel_size)
        }
        WorkspaceLayout::Grid => {
            let cols = ceil_sqrt_usize(count);
            let col = index % cols;
            let row = index / cols;

            let x = origin[0] + WS_INNER_PAD + usize_to_f32(col) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(row) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
    }
}

fn layout_panel_size_from_content(
    layout: WorkspaceLayout,
    count: usize,
    content_size: Option<[f32; 2]>,
) -> Option<[f32; 2]> {
    let content = content_size?;
    let count_f = usize_to_f32(count);

    Some(match layout {
        WorkspaceLayout::Rows => {
            let h = ((content[1] - (count_f - 1.0) * TILE_GAP) / count_f).max(DEFAULT_PANEL_SIZE[1]);
            [content[0].max(DEFAULT_PANEL_SIZE[0]), h]
        }
        WorkspaceLayout::Columns => {
            let w = ((content[0] - (count_f - 1.0) * TILE_GAP) / count_f).max(DEFAULT_PANEL_SIZE[0]);
            [w, content[1].max(DEFAULT_PANEL_SIZE[1])]
        }
        WorkspaceLayout::Grid => {
            let cols = ceil_sqrt_usize(count);
            let rows = count.div_ceil(cols);
            grid_panel_size_from_content(content, cols, rows)
        }
    })
}

/// Compute per-panel size for Grid layout so panels fill the workspace content area.
fn grid_panel_size_from_content(content: [f32; 2], cols: usize, rows: usize) -> [f32; 2] {
    let cols_f = usize_to_f32(cols);
    let rows_f = usize_to_f32(rows);

    let w = ((content[0] - (cols_f - 1.0) * TILE_GAP) / cols_f).max(DEFAULT_PANEL_SIZE[0]);
    let h = ((content[1] - (rows_f - 1.0) * TILE_GAP) / rows_f).max(DEFAULT_PANEL_SIZE[1]);
    [w, h]
}

fn position_occupied(positions: &[[f32; 2]], candidate: [f32; 2]) -> bool {
    positions
        .iter()
        .any(|pos| (pos[0] - candidate[0]).abs() < 1.0 && (pos[1] - candidate[1]).abs() < 1.0)
}

#[derive(Clone, Copy)]
enum ResizeCollisionAxis {
    Horizontal,
    Vertical,
}

type RectCollisionPush = fn([f32; 4], [f32; 4], [f32; 2], f32) -> [f32; 2];

fn resize_expands(delta: [f32; 2]) -> bool {
    delta[0] > f32::EPSILON || delta[1] > f32::EPSILON
}

/// Compute the translation needed to push rect `b` away from rect `a` along
/// `drag_dir` so they no longer overlap, maintaining `gap` pixels of space.
/// Both rects are `[min_x, min_y, max_x, max_y]`.
fn collision_push(a: [f32; 4], b: [f32; 4], drag_dir: [f32; 2], gap: f32) -> [f32; 2] {
    if !rects_overlap(a, b) {
        return [0.0, 0.0];
    }

    let len_sq = drag_dir[0] * drag_dir[0] + drag_dir[1] * drag_dir[1];
    if len_sq < 1e-6 {
        return [0.0, 0.0];
    }
    let len = len_sq.sqrt();
    let dx = drag_dir[0] / len;
    let dy = drag_dir[1] / len;

    // For each axis where the drag has a non-zero component, compute the
    // scalar `t` along the direction vector that would separate the rects
    // on that axis. The minimum such `t` is sufficient because clearing
    // even one axis eliminates the AABB overlap.
    let mut min_t = f32::MAX;

    if dx > 1e-4 {
        let t = (a[2] + gap - b[0]) / dx;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    } else if dx < -1e-4 {
        let t = (a[0] - gap - b[2]) / dx;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    }

    if dy > 1e-4 {
        let t = (a[3] + gap - b[1]) / dy;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    } else if dy < -1e-4 {
        let t = (a[1] - gap - b[3]) / dy;
        if t > 0.0 {
            min_t = min_t.min(t);
        }
    }

    if min_t < f32::MAX {
        [dx * min_t, dy * min_t]
    } else {
        [0.0, 0.0]
    }
}

fn resize_collision_push(a: [f32; 4], b: [f32; 4], resize_delta: [f32; 2], gap: f32) -> [f32; 2] {
    if !rects_overlap(a, b) {
        return [0.0, 0.0];
    }

    for axis in preferred_resize_axes(resize_delta).into_iter().flatten() {
        let push = resize_axis_push(a, b, axis, gap);
        if push[0] != 0.0 || push[1] != 0.0 {
            return push;
        }
    }

    [0.0, 0.0]
}

fn preferred_resize_axes(delta: [f32; 2]) -> [Option<ResizeCollisionAxis>; 2] {
    let horizontal = delta[0] > f32::EPSILON;
    let vertical = delta[1] > f32::EPSILON;

    match (horizontal, vertical) {
        (true, true) if delta[0] >= delta[1] => [
            Some(ResizeCollisionAxis::Horizontal),
            Some(ResizeCollisionAxis::Vertical),
        ],
        (true, true) => [
            Some(ResizeCollisionAxis::Vertical),
            Some(ResizeCollisionAxis::Horizontal),
        ],
        (true, false) => [Some(ResizeCollisionAxis::Horizontal), None],
        (false, true) => [Some(ResizeCollisionAxis::Vertical), None],
        (false, false) => [None, None],
    }
}

fn resize_axis_push(a: [f32; 4], b: [f32; 4], axis: ResizeCollisionAxis, gap: f32) -> [f32; 2] {
    match axis {
        ResizeCollisionAxis::Horizontal => {
            let push = a[2] + gap - b[0];
            if push > 0.0 { [push, 0.0] } else { [0.0, 0.0] }
        }
        ResizeCollisionAxis::Vertical => {
            let push = a[3] + gap - b[1];
            if push > 0.0 { [0.0, push] } else { [0.0, 0.0] }
        }
    }
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    !(a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1])
}
