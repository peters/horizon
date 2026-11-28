//! The presses of the sidebar, applied after the frame that took them: a new work
//! item, a fitted canvas, a move of a workspace, a close and a layout.
//!
//! Every action waits in `SidebarActions` until this apply, so the pass that draws
//! the sidebar never changes the board.

use egui::Context;

use super::super::HorizonApp;
use super::{
    SidebarActions, SidebarWorkspaceInsert, sidebar_workspace_drop_should_dock, sidebar_workspace_insert_dock_side,
};

impl HorizonApp {
    /// The presses of the machine section: Set up, and the button that rents a
    /// machine for the active workspace.
    fn apply_machine_section_presses(&mut self, ctx: &Context, actions: &SidebarActions) {
        if actions.press_set_up {
            self.press_set_up(ctx, horizon_core::SetupGoal::Choose);
        }
        if actions.rent_machine {
            self.rent_a_machine(ctx);
        }
    }

    /// The actions of a press on a machine or on an agent of a machine.
    fn apply_machine_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if let Some(name) = actions.forget_machine.clone() {
            self.forget_machine(&name, ctx);
        }
        if let Some(name) = actions.connect_machine.clone() {
            self.connect_machine(&name, ctx);
        }
        if let Some(agent) = actions.open_machine_agent.clone() {
            self.open_machine_agent(&agent, ctx);
        }
        if let Some((device, panel)) = &actions.open_shared_terminal {
            self.open_shared_terminal(device, panel, ctx);
        }
        if let Some(device) = &actions.mirror_more {
            self.shared_terminals.mirror.expand(device);
        }
        if let Some(key) = &actions.mirror_session {
            self.orchestrator_dock.front_desk_mut().show_mirror(key.clone());
        }
    }
    /// Puts a workspace in the place that a menu chose, which its new terminals
    /// follow and its open panels do not.
    fn apply_workspace_place_actions(&mut self, actions: &SidebarActions) {
        if let Some((workspace_id, place)) = actions.set_workspace_place.clone()
            && self.board.set_workspace_place(workspace_id, place)
        {
            self.mark_runtime_dirty();
        }
        if let Some(name) = actions.put_active_workspace_here.clone()
            && let Some(workspace_id) = self.board.active_workspace
            && self
                .board
                .set_workspace_place(workspace_id, horizon_core::WorkspacePlace::Machine(name))
        {
            self.mark_runtime_dirty();
        }
    }
    pub(super) fn apply_sidebar_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if actions.create_workspace {
            self.create_new_workspace(ctx, None);
        }
        if actions.fit_active_workspace {
            let _ = self.fit_active_workspace(ctx);
        }
        self.apply_machine_section_presses(ctx, actions);
        let workspace_collision_ids = self.workspace_collision_scope(None);
        let workspace_drop_target = actions.workspace_drop.map(|drop| {
            let moved = if sidebar_workspace_drop_should_dock(self.workspace_is_detached(drop.target_workspace_id)) {
                self.board.move_workspace_beside_in_scope(
                    drop.dragged_workspace_id,
                    drop.target_workspace_id,
                    sidebar_workspace_insert_dock_side(drop.insert),
                    &workspace_collision_ids,
                )
            } else {
                false
            };
            let reordered = match drop.insert {
                SidebarWorkspaceInsert::Before => self
                    .board
                    .move_workspace_before(drop.dragged_workspace_id, drop.target_workspace_id),
                SidebarWorkspaceInsert::After => self
                    .board
                    .move_workspace_after(drop.dragged_workspace_id, drop.target_workspace_id),
            };
            if moved || reordered {
                self.mark_runtime_dirty();
            }
            drop.dragged_workspace_id
        });
        self.apply_workspace_place_actions(actions);
        self.apply_machine_actions(ctx, actions);
        if let Some(panel_id) = actions.focus_panel {
            // A click on a row of an open machine or of a panel is the person's,
            // so the terminal of that panel takes the next key.
            self.give_the_keyboard_to(panel_id);
        }

        let pan_to_panel_workspace = actions
            .pan_to_panel
            .and_then(|panel_id| self.board.panel(panel_id).map(|panel| panel.workspace_id));
        let pan_workspace_id = if workspace_drop_target.is_some() {
            workspace_drop_target
        } else if pan_to_panel_workspace.is_some() {
            pan_to_panel_workspace
        } else {
            actions.pan_to_workspace
        };
        if let Some(workspace_id) = pan_workspace_id {
            if actions.pan_to_panel.is_none() {
                self.board.focus_workspace(workspace_id);
            }
            if self.focus_workspace_window(ctx, workspace_id) {
                if let Some(panel_id) = actions.focus_panel {
                    self.board.focus(panel_id);
                }
            } else if let Some(panel_id) = actions.pan_to_panel {
                // Attached canvas: reveal the clicked panel (zooming out when
                // needed) instead of panning to the whole workspace bounds.
                self.reveal_panel_visible(ctx, panel_id);
            } else if let Some((pos, size)) = self.workspace_focus_frame(workspace_id) {
                self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
            }
        }

        if let Some(workspace_id) = actions.detach_workspace {
            self.detach_workspace(workspace_id);
        }
        if let Some(workspace_id) = actions.reattach_workspace {
            self.reattach_workspace(ctx, workspace_id);
        }

        if let Some(panel_id) = actions.close_panel {
            self.request_close_panel(panel_id);
        }
        if let Some(workspace_id) = actions.close_all_in_workspace {
            self.close_workspace_panels(workspace_id);
        }
        if let Some(workspace_id) = actions.remove_empty_workspace {
            let local_id = self
                .board
                .workspace(workspace_id)
                .map(|workspace| workspace.local_id.clone());
            if self.board.remove_empty_workspace(workspace_id) {
                self.mark_runtime_dirty();
                // A workspace that goes takes its conversation and its draft with
                // it: a later row with the same name is another workspace.
                if let Some(local_id) = local_id {
                    self.sidebar_chrome.folds.close(&local_id);
                    self.forget_orchestrator_conversation(&local_id);
                }
            }
        }
        if let Some(workspace_id) = actions.clear_layout
            && self.workspace_can_arrange_panels(workspace_id)
            && self.board.clear_workspace_layout(workspace_id)
        {
            self.mark_runtime_dirty();
        }
        if let Some((workspace_id, layout)) = actions.arrange_layout
            && self.workspace_can_arrange_panels(workspace_id)
        {
            self.board.arrange_workspace(workspace_id, layout);
            self.mark_runtime_dirty();
        }
    }
}
