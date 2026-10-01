use egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use horizon_core::agent_definition;
use horizon_core::assistant::{AssistantAuth, AssistantSettings};

use super::command_bar::{BAR_GAP, BAR_HEIGHT};
use super::icons;
use super::{DEFAULT_WIDTH, HorizonApp, MIN_WIDTH, TOOLBAR_HEIGHT};
use crate::app::util::viewport_local_rect;
use crate::theme;

impl HorizonApp {
    /// Renders the drawer before the canvas so egui reserves its width first.
    pub(in crate::app) fn render_assistant_drawer(&mut self, ui: &mut Ui) {
        // The dock is the assistant's place; the drawer stays shut.
        if super::summon::dock_enabled() {
            self.assistant.open = false;
        }
        self.sync_assistant_focus();
        self.sync_assistant_thread();
        if !self.assistant_visible() {
            return;
        }
        self.close_assistant_if_restarting();
        self.ensure_assistant_panel(ui.ctx());

        let viewport_width = viewport_local_rect(ui).width();
        let default_width = DEFAULT_WIDTH.min(viewport_width * 0.6).max(MIN_WIDTH);
        egui::Panel::right(super::ASSISTANT_PANEL_ID)
            .default_size(default_width)
            .min_size(MIN_WIDTH.min(viewport_width * 0.5))
            .max_size(viewport_width * 0.6)
            .frame(
                Frame::default()
                    .fill(theme::PANEL_BG())
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
            )
            .show(ui, |ui| {
                // The toolbar floats above the window's top edge.
                ui.add_space(TOOLBAR_HEIGHT);
                self.render_drawer_header(ui);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, theme::BORDER_SUBTLE()),
                );
                self.render_drawer_body(ui);
            });
        self.render_assistant_engine_popup(ui.ctx());
        self.render_thread_menu(ui.ctx());
    }

    fn render_drawer_header(&mut self, ui: &mut Ui) {
        let settings = self.assistant.settings;
        let height = 56.0;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let mut header = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(14.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        paint_mark_tile(&mut header);
        header.vertical(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("Assistant").size(14.5).strong().color(theme::FG()));
            ui.label(
                RichText::new(engine_summary(settings))
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
        });
        header.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if header_button(ui, "Close").clicked() {
                self.toggle_assistant();
            }
            if header_button(ui, "New").on_hover_text("Start a new thread").clicked() {
                self.new_assistant_thread();
            }
            let engine = header_button(ui, "Engine");
            if engine.clicked() {
                self.assistant.engine_open = !self.assistant.engine_open;
                self.assistant.draft = self.assistant.settings;
                self.assistant.engine_anchor = Some(engine.rect);
            }
        });
    }

    fn render_drawer_body(&mut self, ui: &mut Ui) {
        let Some(panel_id) = self.board.assistant_panel() else {
            self.render_drawer_waiting(ui);
            return;
        };
        self.render_thread_bar(ui);
        self.render_reach_strip(ui);
        self.render_plan_block(ui);
        self.render_cards_tray(ui);
        let rect = ui.available_rect_before_wrap().shrink2(vec2(10.0, 8.0));
        let bar_rect = Rect::from_min_max(pos2(rect.min.x, rect.max.y - BAR_HEIGHT), rect.max);
        let terminal_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, bar_rect.min.y - BAR_GAP));
        let mut body = ui.new_child(UiBuilder::new().max_rect(terminal_rect));
        let clicked = self.show_assistant_terminal(&mut body, panel_id);
        if clicked {
            self.focus_assistant();
        }
        self.render_command_bar(ui, bar_rect);
        if let Some(reason) = self.assistant.notice.as_deref() {
            body.label(RichText::new(reason).size(12.0).color(theme::PALETTE_YELLOW()));
        }
    }

    /// Shown while there is no agent: the reason it cannot start, or a start in progress.
    fn render_drawer_waiting(&mut self, ui: &mut Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            icons::paint_mark(ui.painter(), rect);
        });
        ui.add_space(14.0);
        Frame::new()
            .fill(theme::BG_ELEVATED())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(16))
            .outer_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match self.assistant.notice.clone() {
                    Some(reason) => {
                        ui.label(RichText::new(reason).size(13.0).color(theme::FG()));
                        ui.add_space(10.0);
                        let choose = ui.button("Choose engine");
                        if choose.clicked() {
                            self.assistant.engine_open = true;
                            self.assistant.engine_anchor = Some(choose.rect);
                            self.assistant.draft = self.assistant.settings;
                        }
                    }
                    None => {
                        ui.label(
                            RichText::new("Starting the assistant...")
                                .size(13.0)
                                .color(theme::FG_SOFT()),
                        );
                    }
                }
            });
    }
}

fn engine_summary(settings: AssistantSettings) -> String {
    let name = agent_definition(settings.agent).map_or("Agent", |agent| agent.display_name);
    let auth = match settings.auth {
        AssistantAuth::Subscription => "Subscription",
        AssistantAuth::ApiKey => "API key",
    };
    format!("{name}  -  {auth}")
}

fn header_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(theme::FG_SOFT()))
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 28.0)),
    )
}

/// The assistant's mark in the header.
fn paint_mark_tile(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
    icons::paint_mark(ui.painter(), rect);
}
