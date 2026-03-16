use egui::{
    Align, Button, Color32, Context, CornerRadius, Id, Layout, Order, Pos2, Rect, Sense, Stroke, UiBuilder, Vec2,
};
use horizon_core::{AttentionSeverity, PanelId, WorkspaceId, WorkspaceLayout};

use crate::theme;

use super::panels::panel_kind_icon;
use super::util;
use super::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT, WS_BG_PAD, WS_TITLE_HEIGHT};

struct WorkspaceSidebarEntry {
    id: WorkspaceId,
    name: String,
    color: Color32,
    is_active: bool,
    panel_ids: Vec<PanelId>,
    attention_count: usize,
}

#[derive(Clone, Copy, Default)]
struct SidebarActions {
    focus_panel: Option<PanelId>,
    pan_to_panel: Option<PanelId>,
    pan_to_workspace: Option<WorkspaceId>,
    close_panel: Option<PanelId>,
    close_all_in_workspace: Option<WorkspaceId>,
    clear_layout: Option<WorkspaceId>,
    arrange_layout: Option<(WorkspaceId, WorkspaceLayout)>,
}

impl HorizonApp {
    pub(super) fn render_toolbar(&mut self, ctx: &Context) {
        let viewport = util::viewport_local_rect(ctx);
        egui::Area::new(Id::new("toolbar"))
            .fixed_pos(viewport.min)
            .constrain(false)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_min_size(Vec2::new(viewport.width(), TOOLBAR_HEIGHT));
                ui.set_max_size(Vec2::new(viewport.width(), TOOLBAR_HEIGHT));
                ui.painter().rect_filled(
                    Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), TOOLBAR_HEIGHT)),
                    CornerRadius::ZERO,
                    theme::TITLEBAR_BG,
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT),
                        Pos2::new(viewport.max.x, viewport.min.y + TOOLBAR_HEIGHT),
                    ],
                    Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 170)),
                );

                let content_rect = Rect::from_min_max(
                    Pos2::new(viewport.min.x + 14.0, viewport.min.y + 8.0),
                    Pos2::new(viewport.max.x - 14.0, viewport.min.y + TOOLBAR_HEIGHT - 8.0),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.label(
                            egui::RichText::new(crate::branding::APP_NAME)
                                .color(theme::FG)
                                .size(14.0)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(crate::branding::APP_TAGLINE)
                                .color(theme::FG_DIM)
                                .size(10.5),
                        );
                        ui.add_space(10.0);

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(8.0);
                            if ui.add(util::chrome_button("Settings")).clicked() {
                                self.toggle_settings();
                            }
                        });
                    },
                );
            });
    }

    pub(super) fn render_sidebar(&mut self, ctx: &Context) {
        if !self.sidebar_visible {
            return;
        }

        let viewport = util::viewport_local_rect(ctx);
        let sidebar_origin = Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT);
        let sidebar_size = Vec2::new(SIDEBAR_WIDTH, viewport.height() - TOOLBAR_HEIGHT);
        let workspace_data = self.sidebar_workspace_data();
        let mut actions = SidebarActions::default();

        egui::Area::new(Id::new("sidebar"))
            .fixed_pos(sidebar_origin)
            .constrain(false)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                Self::paint_sidebar_frame(ui, sidebar_origin, sidebar_size);
                self.render_sidebar_contents(ui, &workspace_data, &mut actions);
            });

        self.apply_sidebar_actions(ctx, &actions);
    }

    fn sidebar_workspace_data(&self) -> Vec<WorkspaceSidebarEntry> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let (r, g, b) = workspace.accent();
                let panel_ids = workspace.panels.clone();
                let attention_count = panel_ids
                    .iter()
                    .filter(|panel_id| self.board.unresolved_attention_for_panel(**panel_id).is_some())
                    .count();

                WorkspaceSidebarEntry {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    color: Color32::from_rgb(r, g, b),
                    is_active: self.board.active_workspace == Some(workspace.id),
                    panel_ids,
                    attention_count,
                }
            })
            .collect()
    }

    fn paint_sidebar_frame(ui: &mut egui::Ui, sidebar_origin: Pos2, sidebar_size: Vec2) {
        ui.set_min_size(sidebar_size);
        ui.set_max_size(sidebar_size);
        ui.painter().rect_filled(
            Rect::from_min_size(sidebar_origin, sidebar_size),
            CornerRadius::ZERO,
            theme::BG_ELEVATED,
        );
        ui.painter().line_segment(
            [
                Pos2::new(sidebar_origin.x + SIDEBAR_WIDTH, sidebar_origin.y),
                Pos2::new(sidebar_origin.x + SIDEBAR_WIDTH, sidebar_origin.y + sidebar_size.y),
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
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("WORKSPACES")
                    .color(theme::FG_DIM)
                    .size(10.5)
                    .strong(),
            );
        });
        ui.add_space(10.0);

        let available = ui.available_height();
        egui::ScrollArea::vertical()
            .max_height(available)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                for workspace in workspace_data {
                    self.render_sidebar_workspace(ui, workspace, workspace_data, actions);
                }
            });

        ui.add_space(8.0);
    }

    fn render_sidebar_workspace(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
    ) {
        ui.add_space(4.0);

        let workspace_response = ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.add_space(14.0);

            let bar_color = if workspace.attention_count > 0 {
                theme::PALETTE_RED
            } else {
                theme::alpha(workspace.color, if workspace.is_active { 240 } else { 110 })
            };
            let bar_rect = ui.allocate_space(Vec2::new(3.0, 22.0)).1;
            ui.painter().rect_filled(bar_rect, CornerRadius::same(2), bar_color);

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(&workspace.name)
                    .color(if workspace.is_active { theme::FG } else { theme::FG_SOFT })
                    .size(13.0)
                    .strong(),
            );
        });

        Self::handle_workspace_click(ui, workspace, &workspace_response.response, actions);
        Self::show_workspace_context_menu(&workspace_response.response, workspace, actions);
        paint_workspace_row_bg(
            ui,
            workspace_response.response.rect,
            workspace.color,
            workspace.is_active,
            workspace_response.response.hovered(),
        );

        ui.add_space(2.0);
        for panel_id in workspace.panel_ids.iter().copied() {
            self.render_sidebar_panel(ui, workspace, workspace_data, panel_id, actions);
        }
        ui.add_space(8.0);
    }

    fn handle_workspace_click(
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        response: &egui::Response,
        actions: &mut SidebarActions,
    ) {
        let interact_id = ui.make_persistent_id(("sidebar_ws", workspace.id.0));
        let click = ui.interact(response.rect, interact_id, Sense::click());
        if click.clicked() {
            if workspace.panel_ids.len() == 1 {
                actions.focus_panel = Some(workspace.panel_ids[0]);
                actions.pan_to_panel = Some(workspace.panel_ids[0]);
            } else {
                actions.pan_to_workspace = Some(workspace.id);
            }
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
            if ui
                .add(
                    Button::new(
                        egui::RichText::new("Close All Terminals")
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
        panel_id: PanelId,
        actions: &mut SidebarActions,
    ) {
        let Some(panel) = self.board.panel(panel_id) else {
            return;
        };
        let title = panel.title.clone();
        let kind = panel.kind;
        let is_focused = self.board.focused == Some(panel_id);
        let attention = self.board.unresolved_attention_for_panel(panel_id).cloned();

        let item_response = ui.vertical(|ui| {
            ui.set_min_height(if attention.is_some() { 46.0 } else { 30.0 });

            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                ui.add_space(30.0);

                let (icon, icon_color) = panel_kind_icon(kind, workspace.color, is_focused);
                ui.label(
                    egui::RichText::new(icon)
                        .color(icon_color)
                        .size(10.0)
                        .monospace()
                        .strong(),
                );
                ui.add_space(4.0);

                let title_width = (ui.available_width() - 28.0).max(48.0);
                ui.add_sized(
                    Vec2::new(title_width, 18.0),
                    egui::Label::new(
                        egui::RichText::new(&title)
                            .color(if is_focused { theme::FG } else { theme::FG_SOFT })
                            .size(12.5),
                    )
                    .truncate(),
                );

                let close =
                    ui.add(Button::new(egui::RichText::new("\u{00D7}").size(16.0).color(theme::FG_DIM)).frame(false));
                if close.clicked() {
                    actions.close_panel = Some(panel_id);
                }
            });

            if let Some(attention_item) = &attention {
                let (label, color) = sidebar_attention_tag(attention_item.severity);
                ui.horizontal(|ui| {
                    ui.add_space(56.0);
                    ui.label(egui::RichText::new(label).size(8.5).color(color).strong());
                    ui.add_space(4.0);
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 14.0),
                        egui::Label::new(
                            egui::RichText::new(&attention_item.summary)
                                .size(9.0)
                                .color(theme::alpha(color, 180)),
                        )
                        .truncate(),
                    );
                });
            }
        });

        let row_clicked =
            item_response.response.interact(Sense::click()).clicked() && actions.close_panel != Some(panel_id);
        if row_clicked {
            actions.focus_panel = Some(panel_id);
            actions.pan_to_panel = Some(panel_id);
        }

        self.show_sidebar_panel_context_menu(
            &item_response.response,
            workspace,
            workspace_data,
            panel_id,
            kind,
            actions,
        );
        paint_panel_row_bg(
            ui,
            item_response.response.rect,
            workspace.color,
            is_focused,
            item_response.response.hovered(),
        );
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
            if kind.is_agent()
                && ui
                    .add(Button::new(egui::RichText::new("Restart").size(12.0).color(theme::FG_SOFT)).frame(false))
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
        if let Some(panel_id) = actions.focus_panel {
            self.board.focus(panel_id);
        }

        let pan_workspace_id = if let Some(panel_id) = actions.pan_to_panel {
            self.board.panel(panel_id).map(|panel| panel.workspace_id)
        } else {
            actions.pan_to_workspace
        };
        if let Some(workspace_id) = pan_workspace_id {
            if actions.pan_to_panel.is_none() {
                self.board.focus_workspace(workspace_id);
            }
            if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                let size = Vec2::new(
                    max[0] - min[0] + 2.0 * WS_BG_PAD,
                    max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                );
                self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
            }
        }

        if let Some(panel_id) = actions.close_panel {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
        }
        if let Some(workspace_id) = actions.close_all_in_workspace {
            let panel_ids: Vec<_> = self
                .board
                .workspace(workspace_id)
                .map(|workspace| workspace.panels.clone())
                .unwrap_or_default();
            for panel_id in panel_ids {
                self.close_panel(panel_id);
                self.panel_screen_rects.remove(&panel_id);
            }
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

fn paint_workspace_row_bg(
    ui: &mut egui::Ui,
    workspace_rect: Rect,
    workspace_color: Color32,
    is_active: bool,
    hovered: bool,
) {
    let workspace_bg = Rect::from_min_max(
        Pos2::new(workspace_rect.min.x + 6.0, workspace_rect.min.y),
        Pos2::new(workspace_rect.max.x - 6.0, workspace_rect.max.y),
    );
    if is_active {
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
