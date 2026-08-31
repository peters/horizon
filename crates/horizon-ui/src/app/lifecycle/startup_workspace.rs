use egui::Context;
use horizon_core::WorkspaceId;

use super::super::HorizonApp;

impl HorizonApp {
    #[profiling::function]
    pub(in crate::app) fn apply_startup_workspace_organization(&mut self, ctx: &Context) -> Option<WorkspaceId> {
        if !std::mem::take(&mut self.startup_workspace_organization_pending) {
            return None;
        }

        let visible_anchor = self.startup_workspace_view_anchor(ctx);
        let Some(alignment) = self.align_attached_workspaces_horizontally(ctx) else {
            tracing::debug!("startup workspace organization skipped: no valid multi-workspace alignment");
            return None;
        };

        tracing::debug!(
            leftmost_workspace_id = alignment.leftmost_workspace.0,
            positions_changed = alignment.positions_changed,
            "startup workspace organization applied"
        );

        // The shortcut animates toward this same target. Session loading
        // applies it immediately so the first interactive frame is already
        // settled and cannot expose the pre-organization camera.
        if let Some(target) = self.pan_target.take() {
            self.canvas_view.set_pan_offset([target.x, target.y]);
            self.mark_runtime_dirty();
        } else if alignment.positions_changed
            && let Some((workspace_id, screen_anchor)) = visible_anchor
        {
            // An empty row head has no bounds from which the shared action can
            // derive a target. Keep a previously visible workspace anchored.
            self.restore_startup_workspace_view_anchor(ctx, workspace_id, screen_anchor);
        }
        self.initial_pan_done = true;
        Some(alignment.leftmost_workspace)
    }

    #[profiling::function]
    pub(in crate::app) fn seed_initial_pan(
        &mut self,
        ctx: &Context,
        aligned_leftmost_workspace: Option<WorkspaceId>,
        preserve_focus: bool,
    ) {
        self.initial_pan_done = true;
        if let Some(workspace_id) = aligned_leftmost_workspace.or_else(|| self.leftmost_workspace_id()) {
            if !preserve_focus {
                self.board.focus_workspace(workspace_id);
            }
            let _ = self.align_initial_view_to_workspace(ctx, workspace_id);
        }
    }
}
