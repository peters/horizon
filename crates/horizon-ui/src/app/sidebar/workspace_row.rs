//! The row of a work item: its fold control, its press, the reveal of a panel and
//! the drag that moves the workspace beside another row.
//!
//! The words and the parts of the row come from `rows`. A drop reads the dock side
//! of the target row here, and the apply moves the dragged workspace.

use egui::{CursorIcon, Pos2, Rect, Sense, Vec2};
use horizon_core::{PanelId, WorkspaceId};

use super::super::HorizonApp;
use super::{
    SidebarActions, SidebarPanelEntry, SidebarWorkspaceDragState, SidebarWorkspaceDropAction, SidebarWorkspaceInsert,
    WorkspaceSidebarEntry, fold, list, rows, text,
};

impl HorizonApp {
    pub(super) fn render_sidebar_workspace(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), rows::ROW_HEIGHT)).1;
        let hovered = ui.rect_contains_pointer(row_rect);
        let open = self.sidebar_chrome.folds.is_open(&workspace.local_id);
        let mark = fold::Mark::of(open, hovered, !workspace.panels.is_empty());
        let word = rows::word_of(&workspace.row);
        let parts = rows::paint(
            ui,
            row_rect,
            workspace,
            word,
            mark,
            hovered,
            self.sidebar_drag_workspace == Some(workspace.id),
        );
        // The control of the fold is a press target of its own, so the row takes no
        // press under it and a press there opens or folds the panel rows alone.
        if let Some(rect) = parts.fold
            && fold::render(ui, rect, &workspace.local_id, mark)
        {
            let accordion = self.template_config.features.sidebar_accordion;
            self.sidebar_chrome.folds.toggle(&workspace.local_id, accordion);
        }

        // The row's own press stops at the fold control.
        let click_right = parts.fold.map_or(row_rect.max.x, |rect| rect.min.x);
        let click_rect = Rect::from_min_max(row_rect.min, Pos2::new(click_right, row_rect.max.y));
        let row_response = ui.interact(
            click_rect,
            ui.make_persistent_id(("sidebar_ws_click", workspace.id.0)),
            Sense::click_and_drag(),
        );
        row_response.widget_info(|| {
            let spoken = text::workspace_spoken(&workspace.name, None, workspace.detached);
            egui::WidgetInfo::labeled(egui::Role::Button, true, spoken)
        });
        // A name or a line that the row cuts shows its whole text in a hover. The
        // hover goes after the row: egui hovers a widget that only senses hover
        // when it lies above the row's click target.
        if parts.name_elided || parts.line_elided {
            let text = if parts.name_elided {
                &workspace.name
            } else {
                &workspace.facts
            };
            ui.interact(
                parts.text,
                ui.make_persistent_id(("sidebar_ws_text", workspace.id.0)),
                Sense::hover(),
            )
            .on_hover_text(text);
        }
        let row_clicked = row_response.clicked();
        self.handle_sidebar_workspace_drag(ui, workspace, row_rect, &row_response, drag_state, hovered);

        if row_clicked {
            // A press on a workspace row shows that workspace in the dock, also
            // while the row is already the active one: the person asks for the
            // conversation of that workspace.
            self.orchestrator_dock.front_desk_mut().show_workspace();
            // With the accordion the panel rows are collapsed, so selecting a
            // workspace reveals its selected panel instead of panning to the
            // whole workspace. Single-panel workspaces behave the same in
            // both modes; flat multi-panel rows keep panning to bounds.
            let accordion = self.template_config.features.sidebar_accordion;
            match self.workspace_row_reveal(accordion, workspace.id, &workspace.panels) {
                Some(panel_id) => {
                    actions.focus_panel = Some(panel_id);
                    actions.pan_to_panel = Some(panel_id);
                }
                None => actions.pan_to_workspace = Some(workspace.id),
            }
        }
        self.show_workspace_context_menu(&row_response, workspace, actions);

        if open {
            let count = workspace.panels.len();
            for (index, panel) in workspace.panels.iter().enumerate() {
                self.render_sidebar_panel(ui, workspace, panel, index + 1 == count, actions);
            }
        }
        ui.add_space(2.0);
    }

    /// The panel a workspace-row click reveals. Accordion rows (where panel
    /// rows are collapsed) and single-panel workspaces reveal a panel;
    /// flat multi-panel rows return `None` so the click keeps its original
    /// pan-to-workspace-bounds behavior.
    pub(super) fn workspace_row_reveal(
        &self,
        accordion: bool,
        workspace_id: WorkspaceId,
        panels: &[SidebarPanelEntry],
    ) -> Option<PanelId> {
        if !accordion && panels.len() > 1 {
            return None;
        }
        self.workspace_reveal_panel(workspace_id, panels)
    }

    /// The panel a workspace-row click reveals: the focused panel when it
    /// belongs to the workspace, otherwise the workspace's first panel.
    pub(super) fn workspace_reveal_panel(
        &self,
        workspace_id: WorkspaceId,
        panels: &[SidebarPanelEntry],
    ) -> Option<PanelId> {
        self.board
            .focused
            .filter(|panel_id| self.board.panel_workspace_id(*panel_id) == Some(workspace_id))
            .or_else(|| panels.first().map(|panel| panel.id))
    }

    fn handle_sidebar_workspace_drag(
        &mut self,
        ui: &egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        row_rect: Rect,
        row_response: &egui::Response,
        drag_state: &mut SidebarWorkspaceDragState,
        click_target_hovered: bool,
    ) {
        if row_response.drag_started() || row_response.dragged() {
            self.sidebar_drag_workspace = Some(workspace.id);
            drag_state.active_this_frame = true;
        }
        if self.sidebar_drag_workspace == Some(workspace.id) && row_response.drag_stopped() {
            drag_state.active_this_frame = true;
            drag_state.drop_requested = true;
        }

        if let Some(dragged_workspace_id) = self.sidebar_drag_workspace.filter(|id| *id != workspace.id)
            && list::accepts_drop(workspace)
            && let Some(pointer_pos) = ui.ctx().pointer_interact_pos()
            && row_rect.expand2(Vec2::new(0.0, 4.0)).contains(pointer_pos)
        {
            let insert = if pointer_pos.y <= row_rect.center().y {
                SidebarWorkspaceInsert::Before
            } else {
                SidebarWorkspaceInsert::After
            };
            drag_state.drop_action = Some(SidebarWorkspaceDropAction {
                dragged_workspace_id,
                target_workspace_id: workspace.id,
                insert,
            });
            rows::paint_workspace_drop_indicator(ui, row_rect, insert, workspace.color);
        }

        if self.sidebar_drag_workspace == Some(workspace.id) && row_response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if click_target_hovered {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }
}
