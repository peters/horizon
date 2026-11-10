//! Where a new workspace appears: **New** in the sidebar header makes an empty one on
//! this computer.
use egui::Context;

use super::super::HorizonApp;

impl HorizonApp {
    /// Makes a workspace at `canvas_pos` or where new workspaces go.
    pub(in crate::app) fn create_new_workspace(&mut self, ctx: &Context, canvas_pos: Option<[f32; 2]>) {
        let name = format!("Workspace {}", self.board.workspaces.len() + 1);
        match canvas_pos {
            Some(position) => self.create_workspace_at_visible(ctx, &name, position),
            None => self.create_workspace_visible(ctx, &name),
        };
        self.mark_runtime_dirty();
    }
}
