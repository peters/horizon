use crate::{layout::point_in_panel, panel::PanelId};

use super::super::Board;

impl Board {
    /// Return the visible sibling whose slot contains the center of an
    /// arranged panel at `position`.
    #[must_use]
    pub fn arranged_panel_collision_target(&self, source: PanelId, position: [f32; 2]) -> Option<PanelId> {
        if !position[0].is_finite() || !position[1].is_finite() {
            return None;
        }

        let source_panel = self.panel(source)?;
        if !self.panel_follows_workspace_layout(source) {
            return None;
        }
        let workspace = self.workspace(source_panel.workspace_id)?;
        workspace.layout?;
        workspace.panel_index(source)?;

        let center = [
            position[0] + source_panel.layout.size[0] * 0.5,
            position[1] + source_panel.layout.size[1] * 0.5,
        ];
        if !center[0].is_finite() || !center[1].is_finite() {
            return None;
        }

        workspace.panels.iter().copied().find(|candidate_id| {
            if *candidate_id == source {
                return false;
            }
            self.panel_follows_workspace_layout(*candidate_id)
                && self.panel(*candidate_id).is_some_and(|candidate| {
                    candidate.workspace_id == source_panel.workspace_id
                        && point_in_panel(center, candidate.layout.position, candidate.layout.size)
                })
        })
    }

    /// Swap two visible panel slots inside the same arranged workspace and
    /// immediately reflow the preset without changing focus or layout mode.
    pub fn swap_arranged_panels(&mut self, source: PanelId, target: PanelId) -> bool {
        if source == target {
            return false;
        }

        let Some(source_panel) = self.panel(source) else {
            return false;
        };
        let workspace_id = source_panel.workspace_id;
        let source_visible = source_panel.visible;
        let target_is_eligible = self
            .panel(target)
            .is_some_and(|panel| panel.visible && panel.workspace_id == workspace_id);
        if !source_visible || !target_is_eligible {
            return false;
        }

        let Some(workspace) = self.workspace(workspace_id) else {
            return false;
        };
        let Some(layout) = workspace.layout else {
            return false;
        };
        let Some(source_index) = workspace.panel_index(source) else {
            return false;
        };
        let Some(target_index) = workspace.panel_index(target) else {
            return false;
        };

        let Some(workspace) = self.workspace_mut(workspace_id) else {
            return false;
        };
        workspace.panels.swap(source_index, target_index);
        self.apply_workspace_layout(workspace_id, layout);
        true
    }
}
