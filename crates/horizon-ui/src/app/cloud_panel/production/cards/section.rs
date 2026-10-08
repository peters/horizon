//! The drawer's building blocks: a titled block of related controls and a row of facts.
use crate::theme;
use egui::{RichText, Stroke};

/// A small heading over a softly framed block, so each tab reads as a few groups.
pub(super) fn show<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.label(RichText::new(title).size(12.0).color(theme::FG_DIM()));
    ui.add_space(4.0);
    frame(ui, add)
}

/// The framed block alone, for content that already names itself.
pub(super) fn frame<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inner = egui::Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(12.0);
    inner
}

/// The width every row button takes, so the buttons of a block line up.
pub(super) const ROW_BUTTON_WIDTH: f32 = 240.0;

/// One action: what it acts on and what it does on the left, its button on the right.
pub(in crate::app::cloud_panel::production) fn row<R>(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        let text = (ui.available_width() - ROW_BUTTON_WIDTH - 16.0).max(120.0);
        ui.allocate_ui_with_layout(egui::vec2(text, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.label(RichText::new(title).size(14.0).color(theme::FG()));
            ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(theme::FG_DIM())).wrap());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().button_padding.x = 12.0;
            add(ui)
        })
        .inner
    })
    .inner
}

/// A row's button, sized like every other row button.
pub(in crate::app::cloud_panel::production) fn row_button(button: egui::Button<'_>) -> egui::Button<'_> {
    button.min_size(egui::vec2(ROW_BUTTON_WIDTH, 30.0))
}

/// Label and value columns; `add` fills the rows with [`fact`].
pub(super) fn facts<R>(ui: &mut egui::Ui, salt: impl egui::AsIdSalt, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Grid::new(salt)
        .num_columns(2)
        .spacing([18.0, 8.0])
        .min_col_width(96.0)
        .show(ui, add)
        .inner
}

pub(super) fn fact_label(ui: &mut egui::Ui, label: &str) {
    ui.add(egui::Label::new(RichText::new(label).size(13.0).color(theme::FG_DIM())).extend());
}

pub(super) fn fact(ui: &mut egui::Ui, label: &str, value: impl Into<RichText>) {
    fact_label(ui, label);
    ui.add(egui::Label::new(value.into().size(14.0).color(theme::FG())).wrap());
    ui.end_row();
}
