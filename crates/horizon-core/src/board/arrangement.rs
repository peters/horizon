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
mod slots;

pub use alignment::WorkspaceAlignment;
pub(crate) use slots::Slot;

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

    /// Arrange panels in a workspace according to a predefined layout.
    /// Panels are equally sized and positioned with gaps.
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

    /// Re-run the selected workspace preset.
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

    #[must_use]
    pub fn workspace_accepts_panel_layout(&self, id: WorkspaceId) -> bool {
        self.workspace(id).is_some()
    }

    pub(super) fn panel_follows_workspace_layout(&self, id: PanelId) -> bool {
        self.panel(id).is_some_and(|panel| panel.visible)
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

    pub(super) fn default_panel_position(
        &self,
        workspace: WorkspaceId,
        size: [f32; 2],
        beside: Option<[f32; 4]>,
    ) -> [f32; 2] {
        if let Some(ws) = self.workspace(workspace) {
            return self.free_tile_position(ws, size, beside);
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
        if self.workspace(id).is_none() {
            return;
        }
        let count = self.arranged_slots(id).len();
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
    /// A selected preset may have been translated away from the origin.
    /// Measure that arrangement's own span so the next preset does not treat
    /// the clearance gap as panel size. Freeform layouts still measure from the
    /// inner-padding edge to the farthest panel edge.
    fn workspace_content_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let workspace = self.workspace(id)?;
        let origin = workspace.position;
        let mut min = [f32::MAX, f32::MAX];
        let mut max = [f32::MIN, f32::MIN];
        let mut any = false;
        for slot in self.arranged_slots(id) {
            if let Some((position, size)) = self.slot_rect(slot) {
                any = true;
                min[0] = min[0].min(position[0]);
                min[1] = min[1].min(position[1]);
                max[0] = max[0].max(position[0] + size[0]);
                max[1] = max[1].max(position[1] + size[1]);
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

    /// The slot size of the current arrangement.
    fn workspace_layout_panel_size(&self, id: WorkspaceId) -> Option<[f32; 2]> {
        let slots = self.arranged_slots(id);
        slots
            .iter()
            .find(|slot| matches!(slot, Slot::Panel(_)))
            .or_else(|| slots.first())
            .and_then(|slot| self.slot_rect(*slot))
            .map(|(_, size)| size)
    }

    pub(crate) fn apply_workspace_layout_with_panel_size(
        &mut self,
        id: WorkspaceId,
        layout: WorkspaceLayout,
        panel_size: [f32; 2],
    ) {
        let Some(origin) = self.workspace(id).map(|workspace| workspace.position) else {
            return;
        };
        let slots = self.arranged_slots(id);
        self.set_workspace_layout(id, Some(layout));
        if slots.is_empty() {
            return;
        }
        let needed = self.place_slots(id, origin, layout, &slots, panel_size);
        if !vec2_eq(needed, panel_size) {
            self.place_slots(id, origin, layout, &slots, needed);
        }
    }

    /// The rectangle `[min_x, min_y, max_x, max_y]` that a panel covers.
    pub(super) fn panel_bounds(&self, id: PanelId) -> Option<[f32; 4]> {
        let panel = self.panel(id)?;
        Some(panel_rect(panel.layout.position, panel.layout.size))
    }

    /// The tile a new panel of `size` fits in without covering another panel
    /// of the workspace.
    ///
    /// A tile is free only when the whole rectangle of the new panel overlaps
    /// no panel of the workspace. Comparing tile positions alone would report
    /// a free tile for a panel that a config, a saved session, a preset or a
    /// person's drag placed somewhere else than the default grid, and the new
    /// panel would cover it.
    ///
    /// `beside` is the rectangle of the panel the new one belongs to, such as
    /// the agent panel of a card. The new panel then stands right beside that
    /// panel, with the top edges aligned, or right below it, with the left edges
    /// aligned, so the pair reads as one group. When both places are taken it
    /// continues the same group: below the column of the anchor with the left
    /// edges level, or beside its row with the top edges level. A panel that
    /// fits in none of these places takes the nearest free tile instead;
    /// without a `beside` panel the first free tile in reading order wins.
    fn free_tile_position(&self, workspace: &Workspace, size: [f32; 2], beside: Option<[f32; 4]>) -> [f32; 2] {
        let origin = workspace.position;
        let occupied: Vec<[f32; 4]> = workspace
            .panels
            .iter()
            .filter_map(|id| self.panel(*id))
            .map(|panel| panel_rect(panel.layout.position, panel.layout.size))
            .collect();
        let Some(beside) = beside else {
            return free_tiles(origin, size, &occupied)
                .first()
                .copied()
                .unwrap_or([origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]);
        };
        if let Some(position) = anchored_positions(beside)
            .into_iter()
            .chain(group_positions(beside, &occupied))
            .find(|position| covers_no_panel(*position, size, &occupied))
        {
            return position;
        }
        free_tiles(origin, size, &occupied)
            .into_iter()
            .min_by(|left, right| {
                center_distance(beside, panel_rect(*left, size))
                    .total_cmp(&center_distance(beside, panel_rect(*right, size)))
            })
            .unwrap_or([origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD])
    }

    /// Returns the visual frame rect `[min_x, min_y, max_x, max_y]` for a
    /// workspace, including the title area and background padding.
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

/// Most tile rows one placement walks, so an extreme coordinate cannot make
/// the walk long.
const MAX_TILE_ROWS: usize = 64;
/// Most tile columns one placement walks, for the same reason.
const MAX_TILE_COLUMNS: usize = 16;

/// The rectangle of a panel at `position` with `size`.
fn panel_rect(position: [f32; 2], size: [f32; 2]) -> [f32; 4] {
    [position[0], position[1], position[0] + size[0], position[1] + size[1]]
}

/// The two places a panel that belongs to the panel `anchor` takes, in order:
/// at the right edge of `anchor` with the top edges aligned, then below it with
/// the left edges aligned.
fn anchored_positions(anchor: [f32; 4]) -> [[f32; 2]; 2] {
    [[anchor[2] + TILE_GAP, anchor[1]], [anchor[0], anchor[3] + TILE_GAP]]
}

/// The two further places of the group that `anchor` stands in, in the same
/// order as [`anchored_positions`]: below every panel of the anchor's column
/// with the left edges level, then beside every panel of its row with the top
/// edges level.
///
/// These keep a new panel in the column or the row of its anchor, which is what
/// makes two terminals that an agent opens for the person line up with its
/// panel after its cards took both places right beside it.
fn group_positions(anchor: [f32; 4], occupied: &[[f32; 4]]) -> [[f32; 2]; 2] {
    let column_bottom = occupied
        .iter()
        .filter(|panel| panel[0] < anchor[2] && anchor[0] < panel[2])
        .fold(anchor[3], |bottom, panel| bottom.max(panel[3]));
    let row_right = occupied
        .iter()
        .filter(|panel| panel[1] < anchor[3] && anchor[1] < panel[3])
        .fold(anchor[2], |right, panel| right.max(panel[2]));
    [[anchor[0], column_bottom + TILE_GAP], [row_right + TILE_GAP, anchor[1]]]
}

/// Whether a panel at `position` with `size` covers no panel of `occupied`.
fn covers_no_panel(position: [f32; 2], size: [f32; 2], occupied: &[[f32; 4]]) -> bool {
    let candidate = panel_rect(position, size);
    !occupied.iter().any(|panel| rects_overlap(candidate, *panel))
}

/// The free tiles that fit a panel of `size`, in reading order.
///
/// The walk covers the tiles near the panels and ends with a place below every
/// panel, which is free whatever the tiles above hold. It is bounded: a
/// workspace holds few panels, so a coordinate that a saved board or a drag
/// left at the edge of the f32 range cannot make the walk long.
fn free_tiles(origin: [f32; 2], size: [f32; 2], occupied: &[[f32; 4]]) -> Vec<[f32; 2]> {
    let column_width = DEFAULT_PANEL_SIZE[0] + TILE_GAP;
    let row_height = DEFAULT_PANEL_SIZE[1] + TILE_GAP;
    let rows = occupied.len().saturating_add(1).min(MAX_TILE_ROWS);
    let columns = occupied.len().saturating_add(1).min(MAX_TILE_COLUMNS);
    let last_bottom = occupied.iter().fold(origin[1], |bottom, rect| bottom.max(rect[3]));

    let mut tiles = Vec::new();
    for row in 0..=rows {
        for column in 0..=columns {
            let position = [
                origin[0] + WS_INNER_PAD + usize_to_f32(column) * column_width,
                origin[1] + WS_INNER_PAD + usize_to_f32(row) * row_height,
            ];
            let candidate = panel_rect(position, size);
            if !occupied.iter().any(|panel| rects_overlap(candidate, *panel)) {
                tiles.push(position);
            }
        }
    }
    let first_row = origin[1] + WS_INNER_PAD;
    tiles.push([origin[0] + WS_INNER_PAD, (last_bottom + TILE_GAP).max(first_row)]);
    tiles
}

/// Squared distance between the centers of two rectangles.
fn center_distance(left: [f32; 4], right: [f32; 4]) -> f32 {
    let dx = f32::midpoint(left[0], left[2]) - f32::midpoint(right[0], right[2]);
    let dy = f32::midpoint(left[1], left[3]) - f32::midpoint(right[1], right[3]);
    dx.mul_add(dx, dy * dy)
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
