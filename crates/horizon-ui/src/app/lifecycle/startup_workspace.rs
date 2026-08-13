use egui::Context;
use horizon_core::WorkspaceId;

use super::super::HorizonApp;

impl HorizonApp {
    #[profiling::function]
    pub(in crate::app) fn apply_startup_workspace_organization(&mut self, ctx: &Context) -> Option<WorkspaceId> {
        if !std::mem::take(&mut self.startup_workspace_organization_pending) {
            return None;
        }

        let visible_anchor = if self.initial_pan_done {
            self.startup_workspace_view_anchor(ctx)
        } else {
            None
        };
        let Some(alignment) =
            super::super::actions::align_attached_workspaces(&mut self.board, &self.detached_workspaces)
        else {
            tracing::debug!("startup workspace organization skipped: no valid multi-workspace alignment");
            return None;
        };

        tracing::debug!(
            leftmost_workspace_id = alignment.leftmost_workspace.0,
            positions_changed = alignment.positions_changed,
            "startup workspace organization applied"
        );
        if alignment.positions_changed {
            if let Some((workspace_id, screen_anchor)) = visible_anchor {
                self.restore_startup_workspace_view_anchor(ctx, workspace_id, screen_anchor);
            } else if self.initial_pan_done && self.startup_workspace_view_anchor(ctx).is_none() {
                let _ = self.align_initial_view_to_workspace(ctx, alignment.leftmost_workspace);
            }
            self.mark_runtime_dirty();
        }
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
