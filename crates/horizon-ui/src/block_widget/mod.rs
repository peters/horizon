//! The body of a card panel: a block document drawn in plain egui.
//!
//! A card shows its components in document order and draws nothing else: no
//! chrome of its own, no inputs, no repaint requests. The person cannot press
//! a button or select a row in this version, so a button is drawn disabled.

use egui::{Color32, RichText, Ui};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, ListRow, Tone};
use horizon_core::PanelId;

use crate::theme;

const CARD_GAP: f32 = 8.0;
const KV_COLUMN_GAP: f32 = 16.0;
const KV_ROW_GAP: f32 = 4.0;
const ROW_GAP: f32 = 2.0;
/// What a card with no components says.
pub(crate) const EMPTY_CARD: &str = "This card is empty.";

/// Draw `document` in document order.
pub(crate) fn show(ui: &mut Ui, panel: PanelId, document: &BlockDocument) {
    egui::ScrollArea::vertical()
        .id_salt(("block_card", panel.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(CARD_GAP);
            if document.components.is_empty() {
                ui.label(RichText::new(EMPTY_CARD).color(theme::FG_DIM()));
                return;
            }
            for (index, component) in document.components.iter().enumerate() {
                if index > 0 {
                    ui.add_space(CARD_GAP);
                }
                show_component(ui, index, component);
            }
        });
}

fn show_component(ui: &mut Ui, index: usize, component: &Component) {
    match component {
        Component::Heading { text } => {
            ui.label(RichText::new(text).heading().color(theme::FG()));
        }
        Component::Text { text } => {
            ui.add(egui::Label::new(RichText::new(text).color(theme::FG_SOFT())).wrap());
        }
        Component::Kv { rows } => show_kv(ui, index, rows),
        Component::Badge { text, tone } => {
            ui.label(RichText::new(text).small().color(tone_color(*tone)));
        }
        Component::Progress { label, percent } => {
            ui.add(
                egui::ProgressBar::new(f32::from(*percent) / 100.0)
                    .fill(theme::ACCENT())
                    .text(label.as_str()),
            );
        }
        Component::Divider => {
            ui.separator();
        }
        Component::List { rows, .. } => show_rows(ui, rows),
        Component::Buttons { items } => show_buttons(ui, items),
    }
}

fn show_kv(ui: &mut Ui, index: usize, rows: &[KvRow]) {
    egui::Grid::new(("block_card_kv", index))
        .num_columns(2)
        .spacing([KV_COLUMN_GAP, KV_ROW_GAP])
        .show(ui, |ui| {
            for row in rows {
                ui.label(RichText::new(&row.label).color(theme::FG_DIM()));
                ui.label(RichText::new(&row.value).color(theme::FG()));
                ui.end_row();
            }
        });
}

fn show_rows(ui: &mut Ui, rows: &[ListRow]) {
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            ui.add_space(ROW_GAP);
        }
        ui.label(RichText::new(&row.title).color(theme::FG()));
        if let Some(subtitle) = &row.subtitle {
            ui.label(RichText::new(subtitle).small().color(theme::FG_DIM()));
        }
    }
}

fn show_buttons(ui: &mut Ui, items: &[Button]) {
    ui.horizontal_wrapped(|ui| {
        for button in items {
            ui.add_enabled(
                false,
                egui::Button::new(RichText::new(&button.label).color(tone_color(button.tone))),
            );
        }
    });
}

fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Neutral => theme::FG_SOFT(),
        Tone::Info => theme::PALETTE_CYAN(),
        Tone::Ok => theme::PALETTE_GREEN(),
        Tone::Warn => theme::PALETTE_YELLOW(),
        Tone::Danger => theme::PALETTE_RED(),
        Tone::Primary => theme::ACCENT(),
    }
}

#[cfg(test)]
mod tests;
