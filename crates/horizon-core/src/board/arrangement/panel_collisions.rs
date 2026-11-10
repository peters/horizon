use crate::layout::TILE_GAP;
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

use super::super::Board;
use super::resize_collision_push;

impl Board {
    /// After a panel is resized, push every overlapping sibling panel within
    /// the same workspace along the dominant resize-growth axis, cascading
    /// until nothing overlaps.
    pub(super) fn resolve_panel_collisions(
        &mut self,
        source: PanelId,
        workspace_id: WorkspaceId,
        resize_delta: [f32; 2],
    ) {
        let bodies = self.collision_bodies(workspace_id);
        // The resized panel stays put. Everything else may be pushed again
        // when a later push lands on it.
        let mut queue = vec![source];
        // Pushes only move panels forward along the growth axis, so the
        // cascade settles; the budget only guards against float drift.
        let mut budget = (bodies.len() + 1).pow(2);

        while let Some(check) = queue.pop() {
            let Some(check_rect) = self.body_rect(check) else {
                continue;
            };

            for &other in &bodies {
                if other == check || other == source {
                    continue;
                }
                let Some(other_rect) = self.body_rect(other) else {
                    continue;
                };

                let push = resize_collision_push(check_rect, other_rect, resize_delta, TILE_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    if budget == 0 {
                        return;
                    }
                    budget -= 1;
                    if let Some(panel) = self.panel_mut(other) {
                        let position = panel.layout.position;
                        panel.move_to([position[0] + push[0], position[1] + push[1]]);
                    }
                    if !queue.contains(&other) {
                        queue.push(other);
                    }
                }
            }
        }
    }

    fn collision_bodies(&self, workspace_id: WorkspaceId) -> Vec<PanelId> {
        self.workspace(workspace_id)
            .map(|workspace| workspace.panels.clone())
            .unwrap_or_default()
    }

    fn body_rect(&self, id: PanelId) -> Option<[f32; 4]> {
        self.panel(id).map(|panel| {
            let [x, y] = panel.layout.position;
            let [width, height] = panel.layout.size;
            [x, y, x + width, y + height]
        })
    }
}
