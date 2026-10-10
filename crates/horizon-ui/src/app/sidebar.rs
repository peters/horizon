mod list;
mod menus;
mod new_workspace;
mod rows;
mod toolbar;

pub(in crate::app) use list::ListCache;
pub(in crate::app) use new_workspace::{NewWorkspace, menu as new_workspace_menu};

use std::collections::HashMap;

use egui::{
    Align, Button, Color32, Context, CornerRadius, CursorIcon, Id, Layout, Order, Pos2, Rect, Sense, Stroke, UiBuilder,
    Vec2,
};
use horizon_core::cloud_list::{self, Group, Row};
use horizon_core::{PanelId, PanelKind, WorkspaceDockSide, WorkspaceId, WorkspaceLayout};

use crate::theme;

use super::panels::panel_kind_icon;
use super::root_chrome::effective_sidebar_width;
use super::util;
use super::workspace::WorkspaceLayoutCapabilities;
use super::{HorizonApp, TOOLBAR_HEIGHT};

struct WorkspaceSidebarEntry {
    id: WorkspaceId,
    name: String,
    color: Color32,
    is_active: bool,
    detached: bool,
    capabilities: WorkspaceLayoutCapabilities,
    panels: Vec<SidebarPanelEntry>,
    /// The workspace's group, status dot and status line in the cloud list.
    row: Row,
}

#[derive(Clone)]
struct SidebarPanelEntry {
    id: PanelId,
    title: String,
    kind: PanelKind,
    is_focused: bool,
}

#[derive(Clone, Copy)]
struct SidebarWorkspaceRowInteraction {
    hovered: bool,
    clicked: bool,
}

#[derive(Clone, Copy, Default)]
struct SidebarActions {
    create_workspace: Option<NewWorkspace>,
    fit_active_workspace: bool,
    workspace_drop: Option<SidebarWorkspaceDropAction>,
    focus_panel: Option<PanelId>,
    pan_to_panel: Option<PanelId>,
    pan_to_workspace: Option<WorkspaceId>,
    detach_workspace: Option<WorkspaceId>,
    reattach_workspace: Option<WorkspaceId>,
    close_panel: Option<PanelId>,
    close_all_in_workspace: Option<WorkspaceId>,
    clear_layout: Option<WorkspaceId>,
    arrange_layout: Option<(WorkspaceId, WorkspaceLayout)>,
}

#[derive(Clone, Copy)]
enum SidebarWorkspaceInsert {
    Before,
    After,
}

#[derive(Clone, Copy)]
struct SidebarWorkspaceDropAction {
    dragged_workspace_id: WorkspaceId,
    target_workspace_id: WorkspaceId,
    insert: SidebarWorkspaceInsert,
}

#[derive(Default)]
struct SidebarWorkspaceDragState {
    active_this_frame: bool,
    drop_requested: bool,
    drop_action: Option<SidebarWorkspaceDropAction>,
}

impl HorizonApp {
    fn has_attached_workspace(&self) -> bool {
        self.board
            .workspaces
            .iter()
            .any(|workspace| !self.workspace_is_detached(workspace.id))
    }

    pub(in crate::app) fn render_sidebar(&mut self, ctx: &Context) {
        if !self.sidebar_visible {
            return;
        }

        let viewport = util::viewport_local_rect(ctx);
        let sidebar_origin = Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT);
        let sidebar_width = effective_sidebar_width(viewport.width());
        let sidebar_size = Vec2::new(sidebar_width, viewport.height() - TOOLBAR_HEIGHT);
        self.refresh_sidebar_rows(std::time::Instant::now());
        let workspace_data = self.sidebar_workspace_data();
        let mut actions = SidebarActions::default();

        egui::Area::new(Id::new("sidebar"))
            .fixed_pos(sidebar_origin)
            .constrain(false)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                Self::paint_sidebar_frame(ui, sidebar_origin, sidebar_size, sidebar_width);
                self.render_sidebar_contents(ui, &workspace_data, &mut actions);
            });

        self.apply_sidebar_actions(ctx, &actions);
    }

    fn sidebar_workspace_data(&self) -> Vec<WorkspaceSidebarEntry> {
        let panel_data = self
            .board
            .panels
            .iter()
            .map(|panel| {
                (
                    panel.id,
                    SidebarPanelEntry {
                        id: panel.id,
                        title: panel.display_title().into_owned(),
                        kind: panel.kind,
                        is_focused: self.board.focused == Some(panel.id),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let panels = workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| panel_data.get(panel_id).cloned())
                    .collect::<Vec<_>>();

                WorkspaceSidebarEntry {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color: theme::workspace_accent(workspace.color_idx),
                    is_active: self.board.active_workspace == Some(workspace.id),
                    detached: self.workspace_is_detached(workspace.id),
                    capabilities: self.workspace_layout_capabilities(workspace.id),
                    panels,
                    row: self.sidebar_row(workspace.id),
                }
            })
            .collect()
    }

    fn paint_sidebar_frame(ui: &mut egui::Ui, sidebar_origin: Pos2, sidebar_size: Vec2, sidebar_width: f32) {
        ui.set_min_size(sidebar_size);
        ui.set_max_size(sidebar_size);
        ui.painter().rect_filled(
            Rect::from_min_size(sidebar_origin, sidebar_size),
            CornerRadius::ZERO,
            theme::BG_ELEVATED(),
        );
        ui.painter().line_segment(
            [
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y),
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y + sidebar_size.y),
            ],
            Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        );
    }

    fn render_sidebar_contents(
        &mut self,
        ui: &mut egui::Ui,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
    ) {
        self.render_sidebar_header(ui, actions);
        ui.add_space(10.0);

        let available = ui.available_height();
        let mut drag_state = SidebarWorkspaceDragState::default();
        egui::ScrollArea::vertical()
            .max_height(available)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                for group in Group::ALL {
                    let rows = workspace_data.iter().filter(|entry| entry.row.group == group);
                    let count = rows.clone().count();
                    if count == 0 {
                        continue;
                    }
                    let summary = cloud_list::group_summary(group, rows.map(|entry| &entry.row));
                    list::render_group_header(ui, group, count, &summary);
                    for workspace in workspace_data.iter().filter(|entry| entry.row.group == group) {
                        self.render_sidebar_workspace(ui, workspace, actions, &mut drag_state);
                    }
                }
            });

        if drag_state.drop_requested {
            actions.workspace_drop = drag_state.drop_action;
            self.sidebar_drag_workspace = None;
        } else if !drag_state.active_this_frame && !ui.ctx().input(|input| input.pointer.primary_down()) {
            self.sidebar_drag_workspace = None;
        }

        ui.add_space(8.0);
    }

    fn render_sidebar_header(&mut self, ui: &mut egui::Ui, actions: &mut SidebarActions) {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("WORKSPACES")
                    .color(theme::FG_DIM())
                    .size(10.5)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                let fit = ui
                    .add_enabled(
                        self.has_attached_workspace(),
                        util::chrome_button("Fit").min_size(Vec2::new(42.0, 24.0)),
                    )
                    .on_hover_text(
                        self.shortcuts
                            .fit_active_workspace
                            .display_label(util::primary_shortcut_label()),
                    );
                if fit.clicked() {
                    actions.fit_active_workspace = true;
                }

                let new_workspace = ui
                    .add(util::chrome_button("New").min_size(Vec2::new(46.0, 24.0)))
                    .on_hover_text("Create a new workspace: in the cloud, or on This PC.");
                if let Some(choice) = new_workspace::menu(&new_workspace, self.new_cloud_workspace_ready()) {
                    actions.create_workspace = Some(choice);
                }
            });
        });
    }

    fn render_sidebar_workspace(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        let accordion = self.template_config.features.sidebar_accordion;
        let compact = rows::is_compact(workspace);
        ui.add_space(if compact { 1.0 } else { 2.0 });

        let height = if compact {
            rows::COMPACT_ROW_HEIGHT
        } else {
            rows::ROW_HEIGHT
        };
        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), height)).1;
        let mut click_target_hovered = ui.rect_contains_pointer(row_rect);
        let mut row_clicked = false;
        rows::paint_workspace_row_bg(
            ui,
            row_rect,
            workspace.color,
            workspace.is_active,
            click_target_hovered,
            self.sidebar_drag_workspace == Some(workspace.id),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(row_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let interaction = rows::render_sidebar_workspace_row_contents(ui, workspace, accordion);
                click_target_hovered |= interaction.hovered;
                row_clicked |= interaction.clicked;
            },
        );

        let row_response = ui.interact(
            row_rect,
            ui.make_persistent_id(("sidebar_ws_click", workspace.id.0)),
            Sense::click_and_drag(),
        );
        click_target_hovered |= row_response.hovered();
        row_clicked |= row_response.clicked();
        self.handle_sidebar_workspace_drag(ui, workspace, row_rect, &row_response, drag_state, click_target_hovered);

        if row_clicked {
            // With the accordion the panel rows are collapsed, so selecting a
            // workspace reveals its selected panel instead of panning to the
            // whole workspace. Single-panel workspaces behave the same in
            // both modes; flat multi-panel rows keep panning to bounds.
            match self.workspace_row_reveal(accordion, workspace.id, &workspace.panels) {
                Some(panel_id) => {
                    actions.focus_panel = Some(panel_id);
                    actions.pan_to_panel = Some(panel_id);
                }
                None => actions.pan_to_workspace = Some(workspace.id),
            }
        }
        Self::show_workspace_context_menu(&row_response, workspace, actions);

        if sidebar_workspace_shows_panels(workspace.is_active, accordion) {
            ui.add_space(2.0);
            for panel in &workspace.panels {
                self.render_sidebar_panel(ui, workspace, panel, actions);
            }
        }
        ui.add_space(if compact { 1.0 } else { 4.0 });
    }

    /// The panel a workspace-row click reveals. Accordion rows (where panel
    /// rows are collapsed) and single-panel workspaces reveal a panel;
    /// flat multi-panel rows return `None` so the click keeps its original
    /// pan-to-workspace-bounds behavior.
    fn workspace_row_reveal(
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
    fn workspace_reveal_panel(&self, workspace_id: WorkspaceId, panels: &[SidebarPanelEntry]) -> Option<PanelId> {
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
            && list::accepts_drop(self.sidebar_row(dragged_workspace_id).group, workspace)
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

    fn render_sidebar_panel(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        panel: &SidebarPanelEntry,
        actions: &mut SidebarActions,
    ) {
        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), 30.0)).1;
        let mut click_target_hovered = ui.rect_contains_pointer(row_rect);
        let mut row_clicked = false;
        rows::paint_panel_row_bg(ui, row_rect, workspace.color, panel.is_focused, click_target_hovered);

        let mut close_clicked = false;
        ui.scope_builder(
            UiBuilder::new().max_rect(row_rect).layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(30.0);
                    ui.add_space(30.0);

                    let (icon, icon_color) = panel_kind_icon(panel.kind, workspace.color, panel.is_focused);
                    let icon_response = ui.add(
                        egui::Label::new(
                            egui::RichText::new(icon)
                                .color(icon_color)
                                .size(10.0)
                                .monospace()
                                .strong(),
                        )
                        .sense(Sense::click()),
                    );
                    click_target_hovered |= icon_response.hovered();
                    row_clicked |= icon_response.clicked();
                    ui.add_space(4.0);

                    let title_width = (ui.available_width() - 28.0).max(48.0);
                    let title_response = ui.add_sized(
                        Vec2::new(title_width, 18.0),
                        egui::Label::new(
                            egui::RichText::new(&panel.title)
                                .color(if panel.is_focused {
                                    theme::FG()
                                } else {
                                    theme::FG_SOFT()
                                })
                                .size(12.5),
                        )
                        .truncate()
                        .sense(Sense::click()),
                    );
                    click_target_hovered |= title_response.hovered();
                    row_clicked |= title_response.clicked();

                    let close = ui.add(
                        Button::new(egui::RichText::new("\u{00D7}").size(16.0).color(theme::FG_DIM())).frame(false),
                    );
                    if close.clicked() {
                        close_clicked = true;
                    }
                });
            },
        );

        let row_click_rect = Rect::from_min_max(row_rect.min, Pos2::new(row_rect.max.x - 28.0, row_rect.max.y));
        let row_response = ui.interact(
            row_click_rect,
            ui.make_persistent_id(("sidebar_panel_click", panel.id.0)),
            Sense::click(),
        );
        click_target_hovered |= row_response.hovered();
        row_clicked |= row_response.clicked();

        if click_target_hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        if close_clicked {
            actions.close_panel = Some(panel.id);
        }

        let row_clicked = row_clicked && !close_clicked;
        if row_clicked {
            actions.focus_panel = Some(panel.id);
            actions.pan_to_panel = Some(panel.id);
        }

        self.show_sidebar_panel_context_menu(&row_response, workspace, panel.id, panel.kind, actions);
        ui.add_space(1.0);
    }

    fn apply_sidebar_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if let Some(choice) = actions.create_workspace {
            self.create_new_workspace(ctx, choice, None);
        }
        if actions.fit_active_workspace {
            let _ = self.fit_active_workspace(ctx);
        }
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
        if let Some(panel_id) = actions.focus_panel {
            self.board.focus(panel_id);
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
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_body_screen_rects.remove(&panel_id);
        }
        if let Some(workspace_id) = actions.close_all_in_workspace {
            self.close_workspace_panels(workspace_id);
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

fn sidebar_workspace_insert_dock_side(insert: SidebarWorkspaceInsert) -> WorkspaceDockSide {
    match insert {
        SidebarWorkspaceInsert::Before => WorkspaceDockSide::Left,
        SidebarWorkspaceInsert::After => WorkspaceDockSide::Right,
    }
}

fn sidebar_workspace_drop_should_dock(target_detached: bool) -> bool {
    !target_detached
}

fn sidebar_workspace_shows_panels(is_active: bool, accordion: bool) -> bool {
    is_active || !accordion
}

#[cfg(test)]
mod tests;
