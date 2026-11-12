use egui::{Align, Button, Color32, Layout, Margin, Stroke};

use super::SettingsAction;
use crate::app::util::{chrome_button, primary_button};
use crate::theme;

/// Draw the footer of the Settings panel: the status line, the buttons of the
/// panel, and the two choices of a value that another computer changed while the
/// person edits the same section.
pub(super) fn render(
    ui: &mut egui::Ui,
    status_text: &str,
    status_color: Color32,
    has_changes: bool,
    conflict: Option<&str>,
) -> SettingsAction {
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
                if conflict.is_some() {
                    // One decision at a time: the two choices of the value that
                    // arrived replace the buttons of the panel until the person
                    // took it or kept the edits of this window.
                    if ui.add(chrome_button("Keep editing")).clicked() {
                        action = SettingsAction::KeepEditing;
                    } else if ui.add(primary_button("Take it")).clicked() {
                        action = SettingsAction::TakeRemote;
                    }
                } else if ui
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
                    let (text, color) = conflict.map_or((status_text, status_color), |_| {
                        (super::CONFLICT_LINE, theme::PALETTE_YELLOW())
                    });
                    if !text.is_empty() {
                        let hover = conflict.map_or_else(|| text.to_owned(), |sections| format!("{text} ({sections})"));
                        ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(12.0)).truncate())
                            .on_hover_text(hover);
                    }
                });
            });
        });

    action
}
