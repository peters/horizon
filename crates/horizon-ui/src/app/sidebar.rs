mod toolbar;

use std::collections::HashMap;

use egui::{
    Align, Button, Color32, Context, CornerRadius, CursorIcon, Id, Layout, Order, Pos2, Rect, Sense, Stroke, UiBuilder,
    Vec2,
};
use horizon_core::{
    AttentionItem, AttentionSeverity, PanelId, PanelKind, WorkspaceDockSide, WorkspaceId, WorkspaceLayout,
};

use crate::theme;

use super::panels::panel_kind_icon;
use super::root_chrome::effective_sidebar_width;
use super::util;
use super::{HorizonApp, TOOLBAR_HEIGHT, WS_BG_PAD, WS_TITLE_HEIGHT};

struct WorkspaceSidebarEntry {
    id: WorkspaceId,
    name: String,
    color: Color32,
    is_active: bool,
    detached: bool,
    panels: Vec<SidebarPanelEntry>,
    attention_count: usize,
}

#[derive(Clone)]
struct SidebarPanelEntry {
    id: PanelId,
    title: String,
    kind: PanelKind,
    is_focused: bool,
    attention: Option<AttentionItem>,
}

#[derive(Clone, Copy, Default)]
struct SidebarActions {
    create_workspace: bool,
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

    pub(super) fn render_sidebar(&mut self, ctx: &Context) {
        if !self.sidebar_visible {
            return;
        }

        let viewport = util::viewport_local_rect(ctx);
        let sidebar_origin = Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT);
        let sidebar_width = effective_sidebar_width(viewport.width());
        let sidebar_size = Vec2::new(sidebar_width, viewport.height() - TOOLBAR_HEIGHT);
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
        let attention_enabled = self.template_config.features.attention_feed;
        let panel_data = self
            .board
            .panels
            .iter()
            .map(|panel| {
                let attention = if attention_enabled {
                    self.board.unresolved_attention_for_panel(panel.id).cloned()
                } else {
                    None
                };
                (
                    panel.id,
                    SidebarPanelEntry {
                        id: panel.id,
                        title: panel.display_title().into_owned(),
                        kind: panel.kind,
                        is_focused: self.board.focused == Some(panel.id),
                        attention,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                let panels = workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| panel_data.get(panel_id).cloned())
                    .collect::<Vec<_>>();
                let attention_count = panels.iter().filter(|panel| panel.attention.is_some()).count();

                WorkspaceSidebarEntry {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color: Color32::from_rgb(r, g, b),
                    is_active: self.board.active_workspace == Some(workspace.id),
                    detached: self.workspace_is_detached(workspace.id),
                    panels,
                    attention_count,
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
            theme::BG_ELEVATED,
        );
        ui.painter().line_segment(
            [
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y),
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y + sidebar_size.y),
            ],
            Stroke::new(1.0, theme::BORDER_SUBTLE),
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

                for workspace in workspace_data {
                    self.render_sidebar_workspace(ui, workspace, workspace_data, actions, &mut drag_state);
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
                    .color(theme::FG_DIM)
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
                    .on_hover_text("Create a new workspace.");
                if new_workspace.clicked() {
                    actions.create_workspace = true;
                }
            });
        });
    }

    fn render_sidebar_workspace(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        ui.add_space(4.0);

        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), 32.0)).1;
        let mut click_target_hovered = ui.rect_contains_pointer(row_rect);
        let mut row_clicked = false;
        paint_workspace_row_bg(
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
                ui.add_space(14.0);

                let bar_color = if workspace.attention_count > 0 {
                    theme::PALETTE_RED
                } else {
                    theme::alpha(workspace.color, if workspace.is_active { 240 } else { 110 })
                };
                let bar_rect = ui.allocate_space(Vec2::new(3.0, 22.0)).1;
                ui.painter().rect_filled(bar_rect, CornerRadius::same(2), bar_color);

                ui.add_space(8.0);
                let name_response = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&workspace.name)
                            .color(if workspace.is_active { theme::FG } else { theme::FG_SOFT })
                            .size(13.0)
                            .strong(),
                    )
                    .sense(Sense::click()),
                );
                click_target_hovered |= name_response.hovered();
                row_clicked |= name_response.clicked();
                if workspace.detached {
                    ui.add_space(4.0);
                    let detached_response = ui.add(
                        egui::Label::new(
                            egui::RichText::new("NEW WINDOW")
                                .color(theme::FG_DIM)
                                .size(8.5)
                                .strong(),
                        )
                        .sense(Sense::click()),
                    );
                    click_target_hovered |= detached_response.hovered();
                    row_clicked |= detached_response.clicked();
                }
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
            if workspace.panels.len() == 1 {
                actions.focus_panel = Some(workspace.panels[0].id);
                actions.pan_to_panel = Some(workspace.panels[0].id);
            } else {
                actions.pan_to_workspace = Some(workspace.id);
            }
        }
        Self::show_workspace_context_menu(&row_response, workspace, actions);

        ui.add_space(2.0);
        for panel in &workspace.panels {
            self.render_sidebar_panel(ui, workspace, workspace_data, panel, actions);
        }
        ui.add_space(8.0);
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

        if sidebar_workspace_drop_should_dock(workspace.detached)
            && let Some(dragged_workspace_id) = self.sidebar_drag_workspace.filter(|id| *id != workspace.id)
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
            paint_workspace_drop_indicator(ui, row_rect, insert, workspace.color);
        }

        if self.sidebar_drag_workspace == Some(workspace.id) && row_response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if click_target_hovered {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }

    fn show_workspace_context_menu(
        response: &egui::Response,
        workspace: &WorkspaceSidebarEntry,
        actions: &mut SidebarActions,
    ) {
        response.context_menu(|ui| {
            ui.set_min_width(160.0);
            ui.label(egui::RichText::new("Arrange Panels").size(11.0).color(theme::FG_DIM));
            if ui
                .add(Button::new(egui::RichText::new("Default").size(12.0).color(theme::FG_SOFT)).frame(false))
                .clicked()
            {
                actions.clear_layout = Some(workspace.id);
                ui.close();
            }
            for layout in WorkspaceLayout::ALL {
                let text = egui::RichText::new(layout.label()).size(12.0).color(theme::FG_SOFT);
                if ui.add(Button::new(text).frame(false)).clicked() {
                    actions.arrange_layout = Some((workspace.id, layout));
                    ui.close();
                }
            }

            ui.separator();
            let detach_label = if workspace.detached {
                "Move to Main Window"
            } else {
                "Open in New Window"
            };
            if ui
                .add(Button::new(egui::RichText::new(detach_label).size(12.0).color(theme::FG_SOFT)).frame(false))
                .clicked()
            {
                if workspace.detached {
                    actions.reattach_workspace = Some(workspace.id);
                } else {
                    actions.detach_workspace = Some(workspace.id);
                }
                ui.close();
            }

            ui.separator();
            if ui
                .add(
                    Button::new(
                        egui::RichText::new("Close All Panels")
                            .size(12.0)
                            .color(theme::PALETTE_RED),
                    )
                    .frame(false),
                )
                .clicked()
            {
                actions.close_all_in_workspace = Some(workspace.id);
                ui.close();
            }
        });
    }

    fn render_sidebar_panel(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        workspace_data: &[WorkspaceSidebarEntry],
        panel: &SidebarPanelEntry,
        actions: &mut SidebarActions,
    ) {
        let row_height = if panel.attention.is_some() { 46.0 } else { 30.0 };
        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), row_height)).1;
        let mut click_target_hovered = ui.rect_contains_pointer(row_rect);
        let mut row_clicked = false;
        paint_panel_row_bg(ui, row_rect, workspace.color, panel.is_focused, click_target_hovered);

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
                                .color(if panel.is_focused { theme::FG } else { theme::FG_SOFT })
                                .size(12.5),
                        )
                        .truncate()
                        .sense(Sense::click()),
                    );
                    click_target_hovered |= title_response.hovered();
                    row_clicked |= title_response.clicked();

                    let close = ui
                        .add(Button::new(egui::RichText::new("\u{00D7}").size(16.0).color(theme::FG_DIM)).frame(false));
                    if close.clicked() {
                        close_clicked = true;
                    }
                });

                if let Some(attention_item) = &panel.attention {
                    let (label, color) = sidebar_attention_tag(attention_item.severity);
                    ui.horizontal(|ui| {
                        ui.add_space(56.0);
                        let tag_response = ui.add(
                            egui::Label::new(egui::RichText::new(label).size(8.5).color(color).strong())
                                .sense(Sense::click()),
                        );
                        click_target_hovered |= tag_response.hovered();
                        row_clicked |= tag_response.clicked();
                        ui.add_space(4.0);
                        let summary_response = ui.add_sized(
                            Vec2::new(ui.available_width(), 14.0),
                            egui::Label::new(
                                egui::RichText::new(&attention_item.summary)
                                    .size(9.0)
                                    .color(theme::alpha(color, 180)),
                            )
                            .truncate()
                            .sense(Sense::click()),
                        );
                        click_target_hovered |= summary_response.hovered();
                        row_clicked |= summary_response.clicked();
                    });
                }
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

        self.show_sidebar_panel_context_menu(&row_response, workspace, workspace_data, panel.id, panel.kind, actions);
        ui.add_space(1.0);
    }

    fn show_sidebar_panel_context_menu(
        &mut self,
        response: &egui::Response,
        workspace: &WorkspaceSidebarEntry,
        workspace_data: &[WorkspaceSidebarEntry],
        panel_id: PanelId,
        kind: horizon_core::PanelKind,
        actions: &mut SidebarActions,
    ) {
        response.context_menu(|ui| {
            ui.set_min_width(160.0);
            ui.label(egui::RichText::new("Move to Workspace").size(11.0).color(theme::FG_DIM));
            for other_workspace in workspace_data {
                if other_workspace.id == workspace.id {
                    continue;
                }
                let text = egui::RichText::new(&other_workspace.name)
                    .size(12.0)
                    .color(theme::FG_SOFT);
                if ui.add(Button::new(text).frame(false)).clicked() {
                    self.board.assign_panel_to_workspace(panel_id, other_workspace.id);
                    self.mark_runtime_dirty();
                    ui.close();
                }
            }

            ui.separator();
            if (kind.is_agent() || kind == horizon_core::PanelKind::Ssh)
                && ui
                    .add(
                        Button::new(
                            egui::RichText::new(if kind == horizon_core::PanelKind::Ssh {
                                "Reconnect"
                            } else {
                                "Restart"
                            })
                            .size(12.0)
                            .color(theme::FG_SOFT),
                        )
                        .frame(false),
                    )
                    .clicked()
            {
                self.panels_to_restart.push(panel_id);
                ui.close();
            }
            if ui
                .add(Button::new(egui::RichText::new("Close").size(12.0).color(theme::PALETTE_RED)).frame(false))
                .clicked()
            {
                actions.close_panel = Some(panel_id);
                ui.close();
            }
        });
    }

    fn apply_sidebar_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if actions.create_workspace {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            self.create_workspace_visible(ctx, &name);
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

        let pan_workspace_id = if workspace_drop_target.is_some() {
            workspace_drop_target
        } else if let Some(panel_id) = actions.pan_to_panel {
            self.board.panel(panel_id).map(|panel| panel.workspace_id)
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
            } else if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                let size = Vec2::new(
                    max[0] - min[0] + 2.0 * WS_BG_PAD,
                    max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                );
                self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
            }
        }

        if let Some(workspace_id) = actions.detach_workspace {
            self.detach_workspace(workspace_id);
        }
        if let Some(workspace_id) = actions.reattach_workspace {
            self.reattach_workspace(workspace_id);
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
            && self.board.clear_workspace_layout(workspace_id)
        {
            self.mark_runtime_dirty();
        }
        if let Some((workspace_id, layout)) = actions.arrange_layout {
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

fn paint_workspace_row_bg(
    ui: &mut egui::Ui,
    workspace_rect: Rect,
    workspace_color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let workspace_bg = Rect::from_min_max(
        Pos2::new(workspace_rect.min.x + 6.0, workspace_rect.min.y),
        Pos2::new(workspace_rect.max.x - 6.0, workspace_rect.max.y),
    );
    if dragging {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace_color, 0.18), 180),
        );
    } else if is_active {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace_color, 0.12), 140),
        );
    } else if hovered {
        ui.painter_at(workspace_bg).rect_filled(
            workspace_bg,
            CornerRadius::same(10),
            theme::alpha(theme::PANEL_BG_ALT, 160),
        );
    }
}

fn paint_workspace_drop_indicator(
    ui: &egui::Ui,
    workspace_rect: Rect,
    insert: SidebarWorkspaceInsert,
    workspace_color: Color32,
) {
    let y = match insert {
        SidebarWorkspaceInsert::Before => workspace_rect.min.y + 1.0,
        SidebarWorkspaceInsert::After => workspace_rect.max.y - 1.0,
    };
    let left = workspace_rect.min.x + 12.0;
    let right = workspace_rect.max.x - 12.0;
    ui.painter().line_segment(
        [Pos2::new(left, y), Pos2::new(right, y)],
        Stroke::new(2.0, theme::alpha(workspace_color, 220)),
    );
}

fn sidebar_attention_tag(severity: AttentionSeverity) -> (&'static str, Color32) {
    match severity {
        AttentionSeverity::High => ("NEEDS INPUT", theme::PALETTE_RED),
        AttentionSeverity::Medium => ("DONE", theme::PALETTE_GREEN),
        AttentionSeverity::Low => ("INFO", theme::ACCENT),
    }
}

fn paint_panel_row_bg(ui: &mut egui::Ui, item_rect: Rect, workspace_color: Color32, is_focused: bool, hovered: bool) {
    let bg_rect = Rect::from_min_max(
        Pos2::new(item_rect.min.x + 6.0, item_rect.min.y),
        Pos2::new(item_rect.max.x - 6.0, item_rect.max.y),
    );
    if is_focused {
        ui.painter_at(bg_rect).rect_filled(
            bg_rect,
            CornerRadius::same(10),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, workspace_color, 0.22), 200),
        );
        let edge = Rect::from_min_size(
            Pos2::new(bg_rect.min.x, bg_rect.min.y + 4.0),
            Vec2::new(2.0, bg_rect.height() - 8.0),
        );
        ui.painter().rect_filled(edge, CornerRadius::same(1), workspace_color);
    } else if hovered {
        ui.painter_at(bg_rect)
            .rect_filled(bg_rect, CornerRadius::same(10), theme::alpha(theme::PANEL_BG_ALT, 180));
    }
}

#[cfg(test)]
mod tests {
    use super::{SidebarWorkspaceInsert, sidebar_workspace_drop_should_dock, sidebar_workspace_insert_dock_side};
    use horizon_core::WorkspaceDockSide;

    #[test]
    fn sidebar_drop_docks_attached_workspace_against_attached_target() {
        assert!(sidebar_workspace_drop_should_dock(false));
    }

    #[test]
    fn sidebar_drop_preserves_detached_workspace_reposition_against_attached_target() {
        assert!(sidebar_workspace_drop_should_dock(false));
    }

    #[test]
    fn sidebar_drop_skips_board_docking_when_target_workspace_is_detached() {
        assert!(!sidebar_workspace_drop_should_dock(true));
    }

    #[test]
    fn sidebar_insert_side_maps_to_expected_dock_side() {
        assert_eq!(
            sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::Before),
            WorkspaceDockSide::Left
        );
        assert_eq!(
            sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::After),
            WorkspaceDockSide::Right
        );
    }
}
