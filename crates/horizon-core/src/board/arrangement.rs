use crate::layout::{
    TILE_GAP, WS_COLLISION_GAP, WS_EMPTY_FRAME_SIZE, WS_FRAME_PAD, WS_FRAME_TOP_EXTRA, WS_INNER_PAD, ceil_sqrt_usize,
    tiled_panel_position, usize_to_f32, workspace_slot_width,
};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelId};
use crate::workspace::{Workspace, WorkspaceId};

use super::{Board, WorkspaceLayout, vec2_eq};

mod alignment;
mod panel_collisions;
mod reordering;

pub use alignment::WorkspaceAlignment;

impl Board {
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

    pub(super) fn push_workspace_colliders_in_direction_in_scope(
        &mut self,
        fixed_workspace_ids: &[WorkspaceId],
        drag_dir: [f32; 2],
        workspace_ids: &[WorkspaceId],
    ) {
        let movable_workspace_ids: Vec<_> = workspace_ids
            .iter()
            .copied()
            .filter(|workspace_id| !fixed_workspace_ids.contains(workspace_id))
            .collect();
        let mut queue = fixed_workspace_ids.to_vec();

        while let Some(check_id) = queue.pop() {
            let Some(check_rect) = self.workspace_frame_rect(check_id) else {
                continue;
            };

            for other_id in &movable_workspace_ids {
                if *other_id == check_id {
                    continue;
                }

                let Some(other_rect) = self.workspace_frame_rect(*other_id) else {
                    continue;
                };
                let push = collision_push(check_rect, other_rect, drag_dir, WS_COLLISION_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    self.translate_workspace(*other_id, push);
                    queue.push(*other_id);
                }
            }
        }
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
        // A cloud member moves inside its cloud. That must not drop the
        // workspace preset, which only arranges panels outside the cloud.
        if let Some(workspace_id) = self.panel_workspace_id(id)
            && !self.panel_is_cloud_member(id)
        {
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
        if self.panel_is_cloud_member(id) {
            if let Some(panel) = self.panel_mut(id) {
                panel.resize_layout(size);
                return true;
            }
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

    /// Arrange panels in a workspace according to a predefined layout.
    /// Panels are equally sized and positioned with gaps.
    ///
    /// Members of a cloud group are left where the cloud's own layout put
    /// them. The workspace preset and the cloud preset are independent.
    ///
    /// Selecting a preset re-arranges immediately, including from manual
    /// placement (default); panel sizes are fitted to the current content
    /// area so the arrangement fills the workspace frame instead of jumping
    /// elsewhere. Fitted sizes are clamped to the default panel size, so an
    /// arrangement can grow the frame; neighboring workspaces are pushed out
    /// of the way like on panel resize. The UI keeps presets selected while
    /// dragging by swapping occupied slots; direct manual moves return the
    /// workspace to freeform placement.
    pub fn arrange_workspace(&mut self, id: WorkspaceId, layout: WorkspaceLayout) {
        let previous_frame = self.workspace_frame_rect(id);
        self.apply_workspace_layout(id, layout);
        self.resolve_workspace_collisions_after_frame_growth(id, previous_frame);
    }

    /// Re-run the selected workspace preset, including clearance around clouds.
    pub fn reapply_workspace_layout_if_set(&mut self, id: WorkspaceId) {
        let Some(layout) = self.workspace_layout_value(id) else {
            return;
        };
        self.apply_workspace_layout(id, layout);
    }

    /// Re-run the preset and push neighboring workspaces if the frame grew.
    pub fn reapply_workspace_layout_resolving_collisions(&mut self, id: WorkspaceId) {
        let previous = self.workspace_frame_rect(id);
        self.reapply_workspace_layout_after(id, previous);
    }

    /// Reapply the preset and resolve growth from a frame captured before a compound update.
    pub fn reapply_workspace_layout_after(&mut self, id: WorkspaceId, previous: Option<[f32; 4]>) {
        self.reapply_workspace_layout_if_set(id);
        self.resolve_workspace_collisions_after_frame_growth(id, previous);
    }

    pub fn clear_workspace_layout(&mut self, id: WorkspaceId) -> bool {
        if self.workspace_layout_value(id).is_none() {
            return false;
        }

        self.set_workspace_layout(id, None);
        true
    }

    /// Workspace presets stay available when a cloud group is attached.
    /// Cloud members are excluded from the arrangement instead of disabling it.
    #[must_use]
    pub fn workspace_accepts_panel_layout(&self, id: WorkspaceId) -> bool {
        self.workspace(id).is_some()
    }

    pub(super) fn panel_is_cloud_member(&self, id: PanelId) -> bool {
        // Builds without cloud frames still restore cloud members as ordinary
        // visible panels. Only a cloud-enabled build should leave them out of
        // the workspace preset.
        #[cfg(feature = "cloud-workspaces")]
        {
            self.panel(id).is_some_and(|panel| {
                crate::runtime_state::cloud_groups::contains_panel(&self.cloud_groups, &panel.local_id)
            })
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = id;
            false
        }
    }

    pub(super) fn panel_follows_workspace_layout(&self, id: PanelId) -> bool {
        self.panel(id).is_some_and(|panel| panel.visible) && !self.panel_is_cloud_member(id)
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
        self.workspace(id)
            .filter(|_| self.workspace_accepts_panel_layout(id))
            .and_then(|workspace| workspace.layout)
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

    pub(crate) fn resolve_workspace_collisions_after_frame_growth(
        &mut self,
        id: WorkspaceId,
        previous_frame: Option<[f32; 4]>,
    ) {
        let workspace_ids: Vec<_> = self.workspaces.iter().map(|workspace| workspace.id).collect();
        self.resolve_workspace_frame_growth_in_scope(id, previous_frame, &workspace_ids);
    }

    /// Push workspaces in `workspace_ids` away from every edge of `id`'s frame
    /// that grew since `previous_frame` (from [`Self::workspace_frame_rect`]).
    pub fn resolve_workspace_frame_growth_in_scope(
        &mut self,
        id: WorkspaceId,
        previous_frame: Option<[f32; 4]>,
        workspace_ids: &[WorkspaceId],
    ) {
        let Some(before) = previous_frame else {
            return;
        };
        let Some(after) = self.workspace_frame_rect(id) else {
            return;
        };

        if after[0] < before[0] - f32::EPSILON {
            self.resolve_workspace_collisions_in_scope(id, [-1.0, 0.0], workspace_ids);
        }
        if after[1] < before[1] - f32::EPSILON {
            self.resolve_workspace_collisions_in_scope(id, [0.0, -1.0], workspace_ids);
        }
        if after[2] > before[2] + f32::EPSILON {
            self.resolve_workspace_collisions_in_scope(id, [1.0, 0.0], workspace_ids);
        }
        if after[3] > before[3] + f32::EPSILON {
            self.resolve_workspace_collisions_in_scope(id, [0.0, 1.0], workspace_ids);
        }
    }

    pub(super) fn apply_workspace_layout(&mut self, id: WorkspaceId, layout: WorkspaceLayout) {
        if !self.workspace_accepts_panel_layout(id) {
            self.set_workspace_layout(id, None);
            return;
        }
        let Some(count) = self.workspace(id).map(|workspace| {
            workspace
                .panels
                .iter()
                .filter(|panel_id| self.panel_follows_workspace_layout(**panel_id))
                .count()
        }) else {
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
    /// bounds (excluding chrome decoration).
    ///
    /// A selected preset may have been translated away from the origin to
    /// clear a cloud. Measure that arrangement's own span so the next preset
    /// does not treat the clearance gap as panel size. Freeform layouts still
    /// measure from the inner-padding edge to the farthest panel edge.
    fn workspace_content_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let workspace = self.workspace(id)?;
        let origin = workspace.position;
        let mut min = [f32::MAX, f32::MAX];
        let mut max = [f32::MIN, f32::MIN];
        let mut any = false;
        for panel_id in &workspace.panels {
            if !self.panel_follows_workspace_layout(*panel_id) {
                continue;
            }
            if let Some(panel) = self.panel(*panel_id) {
                any = true;
                min[0] = min[0].min(panel.layout.position[0]);
                min[1] = min[1].min(panel.layout.position[1]);
                max[0] = max[0].max(panel.layout.position[0] + panel.layout.size[0]);
                max[1] = max[1].max(panel.layout.position[1] + panel.layout.size[1]);
            }
        }
        if !any {
            return None;
        }
        if workspace.layout.is_some() {
            return Some([(max[0] - min[0]).max(0.0), (max[1] - min[1]).max(0.0)]);
        }
        let min_x = origin[0] + WS_INNER_PAD;
        let min_y = origin[1] + WS_INNER_PAD;
        Some([(max[0] - min_x).max(0.0), (max[1] - min_y).max(0.0)])
    }

    fn workspace_layout_panel_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let workspace = self.workspace(id)?;
        workspace.panels.iter().find_map(|panel_id| {
            self.panel_follows_workspace_layout(*panel_id)
                .then(|| self.panel(*panel_id).map(|panel| panel.layout.size))
                .flatten()
        })
    }

    fn apply_workspace_layout_with_panel_size(
        &mut self,
        id: WorkspaceId,
        layout: WorkspaceLayout,
        panel_size: [f32; 2],
    ) {
        let Some((panel_ids, origin)) = self.workspace(id).map(|workspace| {
            (
                workspace
                    .panels
                    .iter()
                    .copied()
                    .filter(|panel_id| self.panel_follows_workspace_layout(*panel_id))
                    .collect::<Vec<_>>(),
                workspace.position,
            )
        }) else {
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
        self.keep_arranged_panels_clear_of_clouds(id, &panel_ids);
    }

    /// A workspace preset must not paint its panels over an attached cloud.
    /// The cloud keeps its own position and internal layout; the arranged
    /// panels move together until their chrome no longer covers it.
    fn keep_arranged_panels_clear_of_clouds(&mut self, id: WorkspaceId, panel_ids: &[PanelId]) {
        let obstacles = self.cloud_overview_rects(id);
        if obstacles.is_empty() {
            return;
        }
        let rects: Vec<[f32; 4]> = panel_ids
            .iter()
            .filter_map(|panel_id| self.panel(*panel_id))
            .map(|panel| super::panel_visual_rect(panel.layout.position, panel.layout.size))
            .collect();
        let shift = clearance_translation(&rects, &obstacles);
        if shift[0].abs() <= f32::EPSILON && shift[1].abs() <= f32::EPSILON {
            return;
        }
        for panel_id in panel_ids {
            if let Some(panel) = self.panel_mut(*panel_id) {
                let position = panel.layout.position;
                panel.move_to([position[0] + shift[0], position[1] + shift[1]]);
            }
        }
    }

    pub(super) fn cloud_overview_rects(&self, id: WorkspaceId) -> Vec<[f32; 4]> {
        #[cfg(feature = "cloud-workspaces")]
        {
            let Some(local_id) = self.workspace(id).map(|workspace| workspace.local_id.clone()) else {
                return Vec::new();
            };
            self.cloud_groups
                .0
                .iter()
                .filter(|group| group.workspace == local_id)
                .map(|group| {
                    let (min, max) = group.overview_bounds();
                    [min[0], min[1], max[0], max[1]]
                })
                .collect()
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = id;
            Vec::new()
        }
    }

    fn first_free_tile_position(&self, workspace: &Workspace) -> [f32; 2] {
        let occupied: Vec<[f32; 2]> = workspace
            .panels
            .iter()
            .filter_map(|id| self.panel(*id))
            .filter(|panel| panel.visible)
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
    /// workspace, including its clouds, the title area and background padding.
    #[must_use]
    pub fn workspace_frame_rect(&self, id: WorkspaceId) -> Option<[f32; 4]> {
        let workspace = self.workspace(id)?;
        if let Some((min, max)) = self.workspace_collision_bounds(id) {
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

pub(crate) fn arranged_panel_layout(
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

pub(super) fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    !(a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1])
}

/// Shortest axis-aligned move of the whole arrangement that leaves every
/// obstacle uncovered. Each candidate is checked against every obstacle, so a
/// step that merely lands on the next cloud is rejected.
fn clearance_translation(rects: &[[f32; 4]], obstacles: &[[f32; 4]]) -> [f32; 2] {
    if rects.is_empty() || obstacles.is_empty() || !arrangement_hits(rects, obstacles) {
        return [0.0, 0.0];
    }
    let (min_x, min_y, max_x, max_y) = arrangement_bounds(rects);
    let mut best: Option<(f32, [f32; 2])> = None;
    let mut consider = |step: [f32; 2]| {
        if step[0].abs() <= f32::EPSILON && step[1].abs() <= f32::EPSILON {
            return;
        }
        if arrangement_hits(&shifted_rects(rects, step), obstacles) {
            return;
        }
        let magnitude = step[0].abs() + step[1].abs();
        if best.is_none_or(|(best_magnitude, _)| magnitude < best_magnitude) {
            best = Some((magnitude, step));
        }
    };
    for obstacle in obstacles {
        consider([obstacle[2] + TILE_GAP - min_x, 0.0]);
        consider([obstacle[0] - TILE_GAP - max_x, 0.0]);
        consider([0.0, obstacle[3] + TILE_GAP - min_y]);
        consider([0.0, obstacle[1] - TILE_GAP - max_y]);
    }
    best.map_or([0.0, 0.0], |(_, step)| step)
}

fn arrangement_hits(rects: &[[f32; 4]], obstacles: &[[f32; 4]]) -> bool {
    rects
        .iter()
        .any(|rect| obstacles.iter().any(|obstacle| rects_overlap(*rect, *obstacle)))
}

fn arrangement_bounds(rects: &[[f32; 4]]) -> (f32, f32, f32, f32) {
    let min_x = rects.iter().map(|rect| rect[0]).fold(f32::MAX, f32::min);
    let min_y = rects.iter().map(|rect| rect[1]).fold(f32::MAX, f32::min);
    let max_x = rects.iter().map(|rect| rect[2]).fold(f32::MIN, f32::max);
    let max_y = rects.iter().map(|rect| rect[3]).fold(f32::MIN, f32::max);
    (min_x, min_y, max_x, max_y)
}

fn shifted_rects(rects: &[[f32; 4]], shift: [f32; 2]) -> Vec<[f32; 4]> {
    rects
        .iter()
        .map(|rect| {
            [
                rect[0] + shift[0],
                rect[1] + shift[1],
                rect[2] + shift[0],
                rect[3] + shift[1],
            ]
        })
        .collect()
}
