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
