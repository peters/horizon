use std::collections::HashMap;

use crate::layout::WS_INNER_PAD;
use crate::workspace::WorkspaceId;

use super::{Board, PANEL_CHROME_PAD, PANEL_CHROME_TITLEBAR};

/// Occupied panel rectangle used by workspace collision. Layout size is the
/// painted body; the title bar and padding extend past that body.
#[must_use]
pub(crate) fn panel_visual_rect(position: [f32; 2], size: [f32; 2]) -> [f32; 4] {
    [
        position[0],
        position[1],
        position[0] + size[0] + 2.0 * PANEL_CHROME_PAD,
        position[1] + size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD,
    ]
}

impl Board {
    /// Computes the bounding rectangle of the visible panels in a workspace.
    /// Returns `(min, max)` in canvas coordinates, or `None` when the
    /// workspace does not exist or has no visible panel.
    #[must_use]
    pub fn workspace_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let workspace = self.workspace(id)?;
        // Anchor min to the workspace origin so the frame doesn't chase
        // panels when they are dragged past the workspace position.
        let origin = workspace.position;
        let mut min = [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD];
        let mut max = [f32::MIN, f32::MIN];
        let mut any = false;
        for panel in self
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == id && panel.visible)
        {
            any = true;
            let rect = panel_visual_rect(panel.layout.position, panel.layout.size);
            min[0] = min[0].min(rect[0]);
            min[1] = min[1].min(rect[1]);
            max[0] = max[0].max(rect[2]);
            max[1] = max[1].max(rect[3]);
        }
        any.then_some((min, max))
    }

    /// Bounds that neighbouring workspaces must clear, matching the frame the
    /// canvas draws.
    pub(super) fn workspace_collision_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        self.workspace_bounds(id)
    }

    /// Computes bounds for every workspace that has a visible panel.
    #[must_use]
    pub fn workspace_bounds_map(&self) -> HashMap<WorkspaceId, ([f32; 2], [f32; 2])> {
        let workspace_origins: HashMap<_, _> = self
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.position))
            .collect();
        let mut bounds = HashMap::with_capacity(workspace_origins.len());

        for panel in &self.panels {
            if !panel.visible {
                continue;
            }
            let Some(origin) = workspace_origins.get(&panel.workspace_id).copied() else {
                continue;
            };
            let entry = bounds.entry(panel.workspace_id).or_insert_with(|| {
                (
                    [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD],
                    [f32::MIN, f32::MIN],
                )
            });
            let rect = panel_visual_rect(panel.layout.position, panel.layout.size);
            entry.0[0] = entry.0[0].min(rect[0]);
            entry.0[1] = entry.0[1].min(rect[1]);
            entry.1[0] = entry.1[0].max(rect[2]);
            entry.1[1] = entry.1[1].max(rect[3]);
        }

        bounds
    }
}
