use egui::{Align, Button, Color32, Layout, Margin, Stroke};

use super::SettingsAction;
use crate::app::util::{chrome_button, primary_button};
use crate::theme;

pub(super) fn render(ui: &mut egui::Ui, status_text: &str, status_color: Color32, has_changes: bool) -> SettingsAction {
    let mut action = SettingsAction::None;

    egui::Panel::bottom(super::SETTINGS_BAR_ID)
        .exact_size(super::SETTINGS_BAR_HEIGHT)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 100))),
        )
        .show(ui, |ui| {
            // The buttons take their place from the right edge first. The status
            // line then keeps the width that they leave, so a long state or a tab
            // with more buttons cuts the line instead of drawing a button over it.
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        Button::new(egui::RichText::new("Close").size(12.0).color(theme::FG_SOFT()))
                            .fill(theme::PANEL_BG_ALT())
                            .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
                            .corner_radius(8),
                    )
                    .clicked()
                {
                    action = SettingsAction::Close;
                } else if has_changes && ui.add(chrome_button("Revert")).clicked() {
                    action = SettingsAction::Revert;
                } else if ui.add(chrome_button("Reset Defaults")).clicked() {
                    action = SettingsAction::ResetDefaults;
                } else if ui.add(primary_button("Save")).clicked() {
                    action = SettingsAction::Save;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if !status_text.is_empty() {
                        ui.add(
                            egui::Label::new(egui::RichText::new(status_text).color(status_color).size(12.0))
                                .truncate(),
                        )
                        .on_hover_text(status_text);
                    }
                });
            });
        });

    action
}
