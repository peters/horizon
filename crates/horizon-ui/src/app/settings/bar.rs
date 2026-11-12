use egui::{Align, Button, Color32, Layout, Margin, Stroke};

use super::SettingsAction;
use crate::app::util::{chrome_button, primary_button};
use crate::theme;

/// Draw the footer of the Settings panel: the status line, the buttons of the
/// panel, and the two choices of a value that another computer changed while the
/// person edits the same section.
///
/// A value that arrived takes two rows and a taller bar: its sentence is the whole
/// question that the person answers, so it has a row of its own above the two
/// choices and no place that they leave can cut it. In one row with both buttons
/// the sentence reads "This setting changed on another ..." at the width of the
/// panel.
pub(super) fn render(
    ui: &mut egui::Ui,
    status_text: &str,
    status_color: Color32,
    has_changes: bool,
    conflict: Option<&str>,
) -> SettingsAction {
    let mut action = SettingsAction::None;
    let height = if conflict.is_some() {
        super::SETTINGS_BAR_CONFLICT_HEIGHT
    } else {
        super::SETTINGS_BAR_HEIGHT
    };

    egui::Panel::bottom(super::SETTINGS_BAR_ID)
        .exact_size(height)
        .frame(
            egui::Frame::default()
                .fill(theme::BG_ELEVATED())
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 100))),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            action = match conflict {
                Some(sections) => conflict_rows(ui, sections),
                None => button_row(ui, status_text, status_color, has_changes),
            }
            .unwrap_or(SettingsAction::None);
        });

    action
}

/// The two rows of a value that arrived: the sentence, and the choices under it.
fn conflict_rows(ui: &mut egui::Ui, sections: &str) -> Option<SettingsAction> {
    // The sentence takes the whole width of the panel, so the width of the row
    // belongs to it alone and a narrow panel wraps it rather than ending it.
    ui.add(
        egui::Label::new(
            egui::RichText::new(super::CONFLICT_LINE)
                .color(theme::PALETTE_YELLOW())
                .size(12.0),
        )
        .wrap(),
    )
    .on_hover_text(format!("{} ({sections})", super::CONFLICT_LINE));
    let mut action = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // One decision at a time: the two choices of the value that arrived replace
        // the buttons of the panel until the person took it or kept the edits of
        // this window.
        if ui.add(chrome_button("Keep editing")).clicked() {
            action = Some(SettingsAction::KeepEditing);
        } else if ui.add(primary_button("Take it")).clicked() {
            action = Some(SettingsAction::TakeRemote);
        }
    });
    action
}

/// The one row of a footer with no value waiting: the buttons, and the status line
/// in the place that they leave.
fn button_row(
    ui: &mut egui::Ui,
    status_text: &str,
    status_color: Color32,
    has_changes: bool,
) -> Option<SettingsAction> {
    let mut action = None;
    // The buttons take their place from the right edge first. The status line then
    // keeps the width that they leave, so a long state or a tab with more buttons
    // cuts the line instead of drawing a button over it.
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
            action = Some(SettingsAction::Close);
        } else if has_changes && ui.add(chrome_button("Revert")).clicked() {
            action = Some(SettingsAction::Revert);
        } else if ui.add(chrome_button("Reset Defaults")).clicked() {
            action = Some(SettingsAction::ResetDefaults);
        } else if ui.add(primary_button("Save")).clicked() {
            action = Some(SettingsAction::Save);
        }
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if !status_text.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(status_text).color(status_color).size(12.0)).truncate())
                    .on_hover_text(status_text);
            }
        });
    });
    action
}
