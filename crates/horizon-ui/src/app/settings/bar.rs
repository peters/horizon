use egui::{Align, Button, Color32, Context, Layout, Margin, Stroke};

use super::SettingsAction;
use crate::app::util::{chrome_button, primary_button};
use crate::theme;

pub(super) fn render(
    ctx: &Context,
    status_text: &str,
    status_color: Color32,
    is_valid: bool,
    has_changes: bool,
) -> SettingsAction {
    let mut action = SettingsAction::None;

    egui::TopBottomPanel::bottom(super::SETTINGS_BAR_ID)
        .exact_height(super::SETTINGS_BAR_HEIGHT)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(24, 8))
                .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 100))),
        )
        .show(ctx, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if !status_text.is_empty() {
                    ui.label(egui::RichText::new(status_text).color(status_color).size(12.0));
                }
                if !is_valid {
                    ui.label(
                        egui::RichText::new("Invalid config")
                            .color(theme::PALETTE_RED())
                            .size(12.0),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(primary_button("Save")).clicked() {
                        action = SettingsAction::Save;
                    } else if ui.add(chrome_button("Reset Defaults")).clicked() {
                        action = SettingsAction::ResetDefaults;
                    } else if has_changes && ui.add(chrome_button("Revert")).clicked() {
                        action = SettingsAction::Revert;
                    } else if ui
                        .add(
                            Button::new(egui::RichText::new("Close").size(12.0).color(theme::FG_SOFT()))
                                .fill(theme::PANEL_BG_ALT())
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                                .corner_radius(8),
                        )
                        .clicked()
                    {
                        action = SettingsAction::Close;
                    }
                });
            });
        });

    action
}
