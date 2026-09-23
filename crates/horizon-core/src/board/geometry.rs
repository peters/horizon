use std::collections::HashMap;

use crate::layout::WS_INNER_PAD;
use crate::workspace::WorkspaceId;

use super::{Board, PANEL_CHROME_PAD, PANEL_CHROME_TITLEBAR};

impl Board {
    /// Computes the bounding rectangle of the visible panels in a workspace
    /// and any attached cloud overview.
    /// Returns `(min, max)` in canvas coordinates, or `None` when the
    /// workspace does not exist or has neither visible panels nor an
    /// attached cloud.
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
            let chrome_w = panel.layout.size[0] + 2.0 * PANEL_CHROME_PAD;
            let chrome_h = panel.layout.size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD;
            min[0] = min[0].min(panel.layout.position[0]);
            min[1] = min[1].min(panel.layout.position[1]);
            max[0] = max[0].max(panel.layout.position[0] + chrome_w);
            max[1] = max[1].max(panel.layout.position[1] + chrome_h);
        }
        // These rects stay aligned with the workspace because
        // `translate_workspace` moves each attached cloud, including its
        // remembered workspace origin, before collision reads this frame.
        for rect in self.cloud_overview_rects(id) {
            any = true;
            min[0] = min[0].min(rect[0]);
            min[1] = min[1].min(rect[1]);
            max[0] = max[0].max(rect[2]);
            max[1] = max[1].max(rect[3]);
        }
        any.then_some((min, max))
    }

    /// Bounds that neighbouring workspaces must clear: the panels plus any
    /// cloud frames and runtime cards, matching the frame the canvas draws.
    pub(super) fn workspace_collision_bounds(&self, id: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let panels = self.workspace_bounds(id);
        #[cfg(feature = "cloud-workspaces")]
        if let Some(clouds) = self.cloud_groups.workspace_extent(self, id) {
            return Some(panels.map_or(clouds, |panels| crate::layout::union_bounds(panels, clouds)));
        }
        panels
    }

    /// Computes bounds for every workspace that has a visible panel or an
    /// attached cloud, in one pass over panels and one pass over clouds.
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
            let chrome_w = panel.layout.size[0] + 2.0 * PANEL_CHROME_PAD;
            let chrome_h = panel.layout.size[1] + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD;
            entry.0[0] = entry.0[0].min(panel.layout.position[0]);
            entry.0[1] = entry.0[1].min(panel.layout.position[1]);
            entry.1[0] = entry.1[0].max(panel.layout.position[0] + chrome_w);
            entry.1[1] = entry.1[1].max(panel.layout.position[1] + chrome_h);
        }

        self.include_cloud_overviews(&workspace_origins, &mut bounds);
        bounds
    }

    fn include_cloud_overviews(
        &self,
        origins: &HashMap<WorkspaceId, [f32; 2]>,
        bounds: &mut HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) {
        #[cfg(feature = "cloud-workspaces")]
        {
            let local_ids: HashMap<&str, WorkspaceId> = self
                .workspaces
                .iter()
                .map(|workspace| (workspace.local_id.as_str(), workspace.id))
                .collect();
            for group in &self.cloud_groups.0 {
                let Some(&workspace_id) = local_ids.get(group.workspace.as_str()) else {
                    continue;
                };
                let Some(&origin) = origins.get(&workspace_id) else {
                    continue;
                };
                let (cloud_min, cloud_max) = group.overview_bounds();
                let entry = bounds.entry(workspace_id).or_insert_with(|| {
                    (
                        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD],
                        [f32::MIN, f32::MIN],
                    )
                });
                entry.0[0] = entry.0[0].min(cloud_min[0]);
                entry.0[1] = entry.0[1].min(cloud_min[1]);
                entry.1[0] = entry.1[0].max(cloud_max[0]);
                entry.1[1] = entry.1[1].max(cloud_max[1]);
            }
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (origins, bounds);
        }
    }
}
