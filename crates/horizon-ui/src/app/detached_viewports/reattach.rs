use egui::Context;

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn finish_workspace_reattachments(
        &mut self,
        ctx: &Context,
        workspace_local_ids: impl IntoIterator<Item = String>,
    ) {
        let mut detached_state_changed = false;
        let mut reattached_workspace_ids = Vec::new();

        for workspace_local_id in workspace_local_ids {
            if self.detached_workspaces.remove(&workspace_local_id).is_some() {
                detached_state_changed = true;
                if let Some(workspace_id) = self.board.workspace_id_by_local_id(&workspace_local_id) {
                    reattached_workspace_ids.push(workspace_id);
                }
            }
            self.pending_detached_window_position_restore
                .remove(&workspace_local_id);
        }

        if !detached_state_changed {
            return;
        }

        if reattached_workspace_ids.is_empty() {
            self.mark_runtime_dirty();
            return;
        }

        if self.template_config.features.organize_workspaces_on_session_load {
            let _ = self.align_attached_workspaces_horizontally(ctx);
        } else {
            let workspace_ids = self.workspace_collision_scope(None);
            for workspace_id in reattached_workspace_ids {
                self.board
                    .separate_workspace_from_overlaps_in_scope(workspace_id, &workspace_ids);
            }
        }
        self.mark_runtime_dirty();
    }

    pub(super) fn process_pending_detached_reattach(&mut self, ctx: &Context) {
        if self.pending_detached_reattach.is_empty() {
            return;
        }

        // Remove pending viewports at the start of the root pass so egui
        // simply stops emitting them this frame.
        let pending = std::mem::take(&mut self.pending_detached_reattach);
        self.finish_workspace_reattachments(ctx, pending);
    }
}
