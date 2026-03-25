use egui::{Align, Color32, Context, CursorIcon, Layout, Margin, RichText, Sense, Stroke};
use horizon_core::StartupPromptReason;

use crate::theme;

use super::util::{chrome_button, primary_button};
use super::{HorizonApp, StartupChooserState};

enum StartupChooserAction {
    None,
    OpenNewSession,
    OpenCopy(String),
    TakeOver(String),
    Resume(String),
}

impl HorizonApp {
    pub(super) fn render_startup_chooser(&mut self, ctx: &Context) {
        let action = {
            let Some(state) = self.startup_chooser.as_mut() else {
                return;
            };
            render_startup_chooser_panel(ctx, state)
        };

        match action {
            StartupChooserAction::None => {}
            StartupChooserAction::OpenNewSession => {
                match self.session_store.create_new_session(&self.template_config) {
                    Ok(session) => self.activate_startup_session(ctx, &session),
                    Err(error) => self.set_startup_error(format!("Failed to create session: {error}")),
                }
            }
            StartupChooserAction::OpenCopy(session_id) => match self.session_store.duplicate_session(&session_id) {
                Ok(session) => self.activate_startup_session(ctx, &session),
                Err(error) => self.set_startup_error(format!("Failed to copy session: {error}")),
            },
            StartupChooserAction::TakeOver(session_id) => match self.session_store.take_over_session(&session_id) {
                Ok(session) => self.activate_startup_session(ctx, &session),
                Err(error) => self.set_startup_error(format!("Failed to take over session: {error}")),
            },
            StartupChooserAction::Resume(session_id) => match self.session_store.resume_session(&session_id) {
                Ok(session) => self.activate_startup_session(ctx, &session),
                Err(error) => self.set_startup_error(format!("Failed to resume session: {error}")),
            },
        }
    }

    fn set_startup_error(&mut self, error: String) {
        if let Some(state) = self.startup_chooser.as_mut() {
            state.error = Some(error);
        }
    }

    fn activate_startup_session(&mut self, ctx: &Context, session: &horizon_core::ResolvedSession) {
        self.activate_persistent_session(session);
        self.restore_window_viewport(ctx);
    }

    pub(super) fn restore_window_viewport(&self, ctx: &Context) {
        let width = self.window_config.width.clamp(800.0, 7680.0);
        let height = self.window_config.height.clamp(600.0, 4320.0);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
        if let (Some(x), Some(y)) = (self.window_config.x, self.window_config.y) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
        }
    }
}

fn render_startup_chooser_panel(ctx: &Context, state: &mut StartupChooserState) -> StartupChooserAction {
    let mut action = StartupChooserAction::None;

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::BG))
        .show(ctx, |ui| {
            render_startup_header(ui, state.chooser.reason);
            ui.add_space(24.0);
            ui.centered_and_justified(|ui| {
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .corner_radius(18)
                    .inner_margin(Margin::same(22))
                    .show(ui, |ui| {
                        ui.set_max_width(760.0);
                        ui.vertical(|ui| {
                            render_config_path(ui, &state.chooser.config_path);
                            ui.add_space(18.0);
                            egui::ScrollArea::vertical()
                                .max_height(360.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| render_session_cards(ui, state));

                            if let Some(error) = &state.error {
                                ui.add_space(6.0);
                                ui.label(RichText::new(error).size(12.0).color(Color32::from_rgb(255, 120, 120)));
                            }

                            ui.add_space(12.0);
                            render_action_row(ui, state, &mut action);
                        });
                    });
            });
        });

    action
}

fn render_startup_header(ui: &mut egui::Ui, reason: StartupPromptReason) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.label(RichText::new("Horizon").size(28.0).strong().color(theme::FG));
        ui.add_space(10.0);
        ui.label(
            RichText::new(match reason {
                StartupPromptReason::LiveConflict => "A Horizon session is already active for this config.",
                StartupPromptReason::MultipleRecoverable => "Multiple recoverable sessions are available.",
            })
            .size(13.0)
            .color(theme::FG_SOFT),
        );
    });
}

fn render_config_path(ui: &mut egui::Ui, config_path: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Config").color(theme::FG).strong());
        ui.add_space(8.0);
        ui.label(RichText::new(config_path).monospace().color(theme::FG_DIM));
    });
}

fn render_session_cards(ui: &mut egui::Ui, state: &mut StartupChooserState) {
    for session in &state.chooser.sessions {
        let selected = state.selected_session_id.as_deref() == Some(session.session_id.as_str());
        if render_session_card(ui, session, selected) {
            state.selected_session_id = Some(session.session_id.clone());
        }
        ui.add_space(10.0);
    }
}

fn render_session_card(ui: &mut egui::Ui, session: &horizon_core::SessionSummary, selected: bool) -> bool {
    let mut radio_clicked = false;
    let frame_response = egui::Frame::default()
        .fill(if selected {
            theme::blend(theme::PANEL_BG, theme::ACCENT, 0.16)
        } else {
            theme::PANEL_BG
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::blend(theme::BORDER_STRONG, theme::ACCENT, 0.75)
            } else {
                theme::BORDER_SUBTLE
            },
        ))
        .corner_radius(14)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                radio_clicked = ui.radio(selected, "").clicked();

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&session.label).size(15.0).strong().color(theme::FG));
                        if session.is_live {
                            ui.add_space(8.0);
                            ui.label(RichText::new("Live").size(11.0).color(theme::ACCENT).strong());
                        }
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} workspaces · {} panels · {}",
                            session.workspace_count,
                            session.panel_count,
                            super::util::format_relative_time(session.last_active_at)
                        ))
                        .size(12.0)
                        .color(theme::FG_SOFT),
                    );
                });
            });
        })
        .response;

    let card_response = ui
        .interact(
            frame_response.rect,
            ui.make_persistent_id(("startup_session_card", &session.session_id)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);

    radio_clicked || card_response.clicked()
}

fn render_action_row(ui: &mut egui::Ui, state: &StartupChooserState, action: &mut StartupChooserAction) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match state.chooser.reason {
        StartupPromptReason::LiveConflict => {
            let selected_session_id = state.selected_session_id.clone();
            if let Some(session_id) = selected_session_id {
                if ui.add(chrome_button("Take Over Session")).clicked() {
                    *action = StartupChooserAction::TakeOver(session_id);
                } else if ui.add(chrome_button("Open Copy")).clicked() {
                    *action = StartupChooserAction::OpenCopy(session_id);
                }
            }

            if ui.add(primary_button("Open New Session")).clicked() {
                *action = StartupChooserAction::OpenNewSession;
            }
        }
        StartupPromptReason::MultipleRecoverable => {
            if let Some(session_id) = state.selected_session_id.clone()
                && ui.add(primary_button("Resume Selected")).clicked()
            {
                *action = StartupChooserAction::Resume(session_id);
            }
            if ui.add(chrome_button("Open New Session")).clicked() {
                *action = StartupChooserAction::OpenNewSession;
            }
        }
    });
}
