use egui::{
    Align, Align2, Context, CornerRadius, FontId, Id, Layout, Order, Pos2, Rect, Sense, Stroke, UiBuilder, Vec2,
};

use crate::app::root_chrome::{
    ROOT_TOOLBAR_BUTTON_GAP, ROOT_TOOLBAR_BUTTON_HEIGHT, ROOT_TOOLBAR_FPS_WIDTH, RootToolbarLayout, ToolbarAction,
    ToolbarItem, root_toolbar_layout,
};
use crate::app::util;
use crate::app::{HorizonApp, TOOLBAR_HEIGHT};
use crate::{branding, theme};

impl HorizonApp {
    pub(in crate::app) fn render_toolbar(&mut self, ctx: &Context) {
        let viewport = util::viewport_local_rect(ctx);
        let layout = root_toolbar_layout(viewport);

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

                Self::render_toolbar_brand(ui, &layout);
                self.render_toolbar_search_rect(ui, &layout);
                self.render_toolbar_actions(ui, &layout);
            });
    }

    fn render_toolbar_brand(ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.brand_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.label(
                    egui::RichText::new(branding::APP_NAME)
                        .color(theme::FG)
                        .size(14.0)
                        .strong(),
                );
                if layout.show_tagline {
                    ui.add_space(ROOT_TOOLBAR_BUTTON_GAP);
                    ui.label(
                        egui::RichText::new(branding::APP_TAGLINE)
                            .color(theme::FG_DIM)
                            .size(10.5),
                    );
                }
            },
        );
    }

    fn render_toolbar_search_rect(&mut self, ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        let mut search_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(layout.search_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        self.render_toolbar_search(&mut search_ui);
    }

    fn render_toolbar_actions(&mut self, ui: &mut egui::Ui, layout: &RootToolbarLayout) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.actions_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = ROOT_TOOLBAR_BUTTON_GAP;

                for item in &layout.visible_items {
                    match *item {
                        ToolbarItem::FpsMeter => self.render_toolbar_fps_meter(ui),
                        ToolbarItem::Action(action) => self.render_toolbar_action_button(ui, action),
                        ToolbarItem::OverflowMenu => self.render_toolbar_overflow_menu(ui, &layout.overflow_actions),
                    }
                }
            },
        );
    }

    fn render_toolbar_fps_meter(&self, ui: &mut egui::Ui) {
        let stats = self.frame_stats.snapshot();
        let value = if stats.sample_count == 0 {
            "0".to_string()
        } else {
            format!("{:.0}", stats.fps)
        };
        let accent = if stats.sample_count == 0 {
            theme::BORDER_SUBTLE
        } else if stats.fps >= 100.0 {
            theme::PALETTE_GREEN
        } else if stats.fps >= 60.0 {
            theme::ACCENT
        } else {
            theme::PALETTE_RED
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(fps_meter_width(), 24.0), Sense::hover());
        let painter = ui.painter();
        let stroke_color = theme::alpha(theme::blend(theme::BORDER_SUBTLE, accent, 0.36), 220);
        let fill_color = theme::alpha(theme::blend(theme::PANEL_BG_ALT, accent, 0.10), 232);
        let dot_center = Pos2::new(rect.min.x + 10.0, rect.center().y);

        painter.rect_filled(rect, CornerRadius::same(10), fill_color);
        painter.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0, stroke_color),
            egui::StrokeKind::Outside,
        );
        painter.circle_filled(dot_center, 3.0, theme::alpha(accent, 230));
        painter.text(
            Pos2::new(rect.min.x + 18.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            FontId::monospace(11.5),
            theme::FG,
        );
        painter.text(
            Pos2::new(rect.max.x - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            "fps",
            FontId::proportional(8.5),
            theme::alpha(theme::FG_DIM, 220),
        );

        let tooltip = if stats.sample_count == 0 {
            "Idle. The meter resumes once Horizon redraws again.".to_string()
        } else {
            format!(
                "{:.0} FPS average over {} frames ({:.2} ms/frame)",
                stats.fps, stats.sample_count, stats.frame_time_ms
            )
        };
        let _ = response.on_hover_text(tooltip);
    }

    fn render_toolbar_action_button(&mut self, ui: &mut egui::Ui, action: ToolbarAction) {
        let response = match action {
            ToolbarAction::FitWorkspace => ui
                .add_enabled(
                    self.has_attached_workspace(),
                    util::chrome_button(action.label())
                        .min_size(Vec2::new(action_button_width(action), ROOT_TOOLBAR_BUTTON_HEIGHT)),
                )
                .on_hover_text(
                    self.shortcuts
                        .fit_active_workspace
                        .display_label(util::primary_shortcut_label()),
                ),
            ToolbarAction::QuickNav => ui
                .add(
                    util::chrome_button(action.label())
                        .min_size(Vec2::new(action_button_width(action), ROOT_TOOLBAR_BUTTON_HEIGHT)),
                )
                .on_hover_text(
                    self.shortcuts
                        .command_palette
                        .display_label(util::primary_shortcut_label()),
                ),
            ToolbarAction::RemoteHosts => ui
                .add(
                    util::chrome_button(action.label())
                        .min_size(Vec2::new(action_button_width(action), ROOT_TOOLBAR_BUTTON_HEIGHT)),
                )
                .on_hover_text(
                    self.shortcuts
                        .open_remote_hosts
                        .display_label(util::primary_shortcut_label()),
                ),
            ToolbarAction::NewWorkspace | ToolbarAction::Settings => ui.add(
                util::chrome_button(action.label())
                    .min_size(Vec2::new(action_button_width(action), ROOT_TOOLBAR_BUTTON_HEIGHT)),
            ),
        };

        if response.clicked() {
            self.perform_toolbar_action(ui.ctx(), action);
        }
    }

    fn render_toolbar_overflow_menu(&mut self, ui: &mut egui::Ui, overflow_actions: &[ToolbarAction]) {
        ui.scope(|ui| {
            ui.style_mut().spacing.button_padding = Vec2::new(12.0, 7.0);
            ui.menu_button(egui::RichText::new("More").size(11.0).color(theme::FG_SOFT), |ui| {
                ui.set_min_width(160.0);

                for action in overflow_actions {
                    let mut button =
                        egui::Button::new(egui::RichText::new(action.label()).size(12.0).color(theme::FG_SOFT))
                            .frame(false);

                    if *action == ToolbarAction::FitWorkspace {
                        button = button.sense(if self.has_attached_workspace() {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        });
                    }

                    let response = if *action == ToolbarAction::FitWorkspace {
                        ui.add_enabled(self.has_attached_workspace(), button)
                    } else {
                        ui.add(button)
                    };

                    if response.clicked() {
                        self.perform_toolbar_action(ui.ctx(), *action);
                        ui.close();
                    }
                }
            });
        });
    }

    fn perform_toolbar_action(&mut self, ctx: &Context, action: ToolbarAction) {
        match action {
            ToolbarAction::NewWorkspace => {
                let name = format!("Workspace {}", self.board.workspaces.len() + 1);
                self.create_workspace_visible(ctx, &name);
            }
            ToolbarAction::QuickNav => self.open_command_palette(),
            ToolbarAction::FitWorkspace => {
                self.execute_command(ctx, &crate::command_registry::CommandId::FitActiveWorkspace);
            }
            ToolbarAction::RemoteHosts => self.toggle_remote_hosts_overlay(ctx),
            ToolbarAction::Settings => self.toggle_settings(),
        }
    }
}

fn fps_meter_width() -> f32 {
    ROOT_TOOLBAR_FPS_WIDTH
}

fn action_button_width(action: ToolbarAction) -> f32 {
    match action {
        ToolbarAction::NewWorkspace => 128.0,
        ToolbarAction::QuickNav => 102.0,
        ToolbarAction::FitWorkspace => 126.0,
        ToolbarAction::RemoteHosts => 120.0,
        ToolbarAction::Settings => 92.0,
    }
}
