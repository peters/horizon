//! A production cloud in its workspace's preset: one slot, sized like the panels beside it.
use super::{CloudGroup, CloudGroups};
use crate::Board;
use crate::board::Slot;

/// The smallest member a cloud fitted to a slot keeps, the same as the smallest panel.
pub const SLOT_MIN_MEMBER: [f32; 2] = [320.0, 220.0];

impl CloudGroup {
    /// A deployed cloud that is not collapsed takes a slot when its workspace has a
    /// preset. Prototype clouds keep their runtime card beside the frame, so they stay
    /// obstacles the preset arranges around.
    #[must_use]
    pub fn takes_workspace_slot(&self) -> bool {
        self.remote.is_some() && !self.collapsed
    }

    /// Put the frame at `position` and fit it to `size`. Members move with the frame,
    /// and a cloud with its own preset arranges them in the new size.
    pub(crate) fn fit_slot(&mut self, board: &mut Board, workspace: [f32; 2], position: [f32; 2], size: [f32; 2]) {
        self.workspace_position = workspace;
        let delta = [position[0] - self.position[0], position[1] - self.position[1]];
        self.shift_in_slot(board, delta);
        self.apply_frame_size(board, size, SLOT_MIN_MEMBER);
    }

    /// Move the frame and its members without reapplying the workspace preset.
    pub(crate) fn shift_in_slot(&mut self, board: &mut Board, delta: [f32; 2]) {
        for (coordinate, amount) in self.position.iter_mut().zip(delta) {
            *coordinate += amount;
        }
        for panel in &mut board.panels {
            if self.panels.contains(&panel.local_id) {
                for (coordinate, amount) in panel.layout.position.iter_mut().zip(delta) {
                    *coordinate += amount;
                }
            }
        }
    }
}

impl CloudGroups {
    /// Take the board's geometry for clouds that sit in a preset slot, so a preset applied
    /// to the board alone is not undone by this list's next reconcile.
    pub fn adopt_slot_geometry(&mut self, board: &Board) {
        for group in &mut self.0 {
            let Some(placed) = board
                .cloud_groups
                .0
                .iter()
                .find(|other| other.environment.id == group.environment.id)
            else {
                continue;
            };
            if !board.cloud_takes_slot(placed) {
                continue;
            }
            group.position = placed.position;
            group.size = placed.size;
            group.workspace_position = placed.workspace_position;
            group.slot = placed.slot;
        }
    }
}

impl Board {
    /// Whether `group` sits in a slot of its workspace's preset. Only a cloud the board
    /// has registered is placed by the preset.
    #[must_use]
    pub fn cloud_takes_slot(&self, group: &CloudGroup) -> bool {
        group.takes_workspace_slot()
            && self
                .cloud_groups
                .0
                .iter()
                .any(|placed| placed.environment.id == group.environment.id)
            && self
                .workspace_id_by_local_id(&group.workspace)
                .and_then(|id| self.workspace(id))
                .is_some_and(|workspace| workspace.layout.is_some())
    }

    /// Resize the slot of the cloud `environment`: every slot of its preset takes `size`,
    /// as when a panel there is resized. Neighbouring workspaces move out of the way.
    pub fn resize_cloud_slot(&mut self, environment: &str, size: [f32; 2]) -> bool {
        let Some((workspace, layout)) = self.slot_workspace(environment) else {
            return false;
        };
        if !size.iter().all(|value| value.is_finite() && *value > 0.0) {
            return false;
        }
        let previous = self.workspace_frame_rect(workspace);
        self.apply_workspace_layout_with_panel_size(workspace, layout, size);
        self.resolve_workspace_collisions_after_frame_growth(workspace, previous);
        true
    }

    /// Move the slot of the cloud `environment` onto the slot under `point`.
    pub fn swap_cloud_slot_at(&mut self, environment: &str, point: [f32; 2]) -> bool {
        let Some((workspace, _)) = self.slot_workspace(environment) else {
            return false;
        };
        let Some(index) = self
            .cloud_groups
            .0
            .iter()
            .position(|group| group.environment.id == environment)
        else {
            return false;
        };
        self.swap_slot_at(workspace, Slot::Cloud(index), point)
    }

    fn slot_workspace(&self, environment: &str) -> Option<(crate::WorkspaceId, crate::WorkspaceLayout)> {
        let group = self
            .cloud_groups
            .0
            .iter()
            .find(|group| group.environment.id == environment)?;
        if !self.cloud_takes_slot(group) {
            return None;
        }
        let workspace = self.workspace_id_by_local_id(&group.workspace)?;
        Some((workspace, self.workspace(workspace)?.layout?))
    }
}
