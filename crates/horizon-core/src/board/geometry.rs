use std::collections::HashMap;

use crate::layout::WS_INNER_PAD;
use crate::workspace::WorkspaceId;

use super::{Board, PANEL_CHROME_PAD, PANEL_CHROME_TITLEBAR};

impl Board {
    /// Computes the bounding rectangle of all panels in a workspace.
    /// Returns `(min, max)` in canvas coordinates, or `None` when the
    /// workspace is empty or does not exist.
    #[must_use]
    pub fn workspace_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let workspace = self.workspace(id)?;
        let mut panels = self.panels.iter().filter(|panel| panel.workspace_id == id).peekable();
        panels.peek()?;

        // Anchor min to the workspace origin so the frame doesn't chase
        // panels when they are dragged past the workspace position.
        let origin = workspace.position;
        let mut min = [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD];
        let mut max = [f32::MIN, f32::MIN];
        for panel in panels {
            let chrome_w = panel.layout.size[0] + 2.0 * PANEL_CHROME_PAD;
            let chrome_h = panel.layout.size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD;
            min[0] = min[0].min(panel.layout.position[0]);
            min[1] = min[1].min(panel.layout.position[1]);
            max[0] = max[0].max(panel.layout.position[0] + chrome_w);
            max[1] = max[1].max(panel.layout.position[1] + chrome_h);
        }

        Some((min, max))
    }

    /// Computes bounds for every non-empty workspace in one pass over panels.
    #[must_use]
    pub fn workspace_bounds_map(&self) -> HashMap<WorkspaceId, ([f32; 2], [f32; 2])> {
        let workspace_origins: HashMap<_, _> = self
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.position))
            .collect();
        let mut bounds = HashMap::with_capacity(workspace_origins.len());

        for panel in &self.panels {
            let Some(origin) = workspace_origins.get(&panel.workspace_id).copied() else {
                continue;
            };
            let entry = bounds.entry(panel.workspace_id).or_insert_with(|| {
                (
                    [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD],
                    [f32::MIN, f32::MIN],
                )
            });
            let chrome_w = panel.layout.size[0] + 2.0 * PANEL_CHROME_PAD;
            let chrome_h = panel.layout.size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD;
            entry.0[0] = entry.0[0].min(panel.layout.position[0]);
            entry.0[1] = entry.0[1].min(panel.layout.position[1]);
            entry.1[0] = entry.1[0].max(panel.layout.position[0] + chrome_w);
            entry.1[1] = entry.1[1].max(panel.layout.position[1] + chrome_h);
        }

        bounds
    }
}
