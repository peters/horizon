//! The body of a card panel: a block document drawn in plain egui.
//!
//! A card shows its components in document order and draws nothing else: no
//! chrome of its own and no repaint loop of its own. A button and a selectable
//! row are live while the agent that owns the card is in the workspace: a press
//! changes the row selection, records a click for that agent and asks for the
//! one repaint that settles it. Without the agent, everything is drawn disabled
//! and the card says so.

use egui::{Color32, RichText, Ui};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, ListRow, Selection, Tone};
use horizon_core::{CardRuntime, PanelId};

use crate::theme;

const CARD_GAP: f32 = 8.0;
const KV_COLUMN_GAP: f32 = 16.0;
const KV_ROW_GAP: f32 = 4.0;
const ROW_GAP: f32 = 2.0;
/// What a card with no components says.
pub(crate) const EMPTY_CARD: &str = "This card is empty.";
/// What a card says while no agent panel of its owner is in the workspace.
pub(crate) const AGENT_ABSENT: &str = "Its agent is not here.";
/// What a card says while a click did not fit its queue.
pub(crate) const CLICKS_REFUSED: &str = "Too many clicks are waiting.";

/// One card as the person sees it this frame.
pub(crate) struct CardView<'a> {
    /// The document to draw.
    pub(crate) document: &'a BlockDocument,
    /// The clicks and the row selection of this card.
    pub(crate) card: &'a mut CardRuntime,
    /// The revision the person sees, which a click reports back.
    pub(crate) revision: u64,
    /// An agent panel of the owner is in the card's workspace.
    pub(crate) agent_present: bool,
    /// No host dialog covers the card, so a press counts.
    pub(crate) interactive: bool,
    /// Wall-clock time of this frame, in milliseconds.
    pub(crate) now_millis: i64,
}

/// Draw `view` in document order.
pub(crate) fn show(ui: &mut Ui, panel: PanelId, mut view: CardView<'_>) {
    let enabled = view.agent_present && view.interactive;
    let refused = view.card.refused_notice(view.now_millis);
    egui::ScrollArea::vertical()
        .id_salt(("block_card", panel.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(CARD_GAP);
            if !view.agent_present {
                ui.label(RichText::new(AGENT_ABSENT).color(theme::FG_DIM()));
            }
            if refused {
                ui.label(RichText::new(CLICKS_REFUSED).color(theme::FG_DIM()));
            }
            let document = view.document;
            if document.components.is_empty() {
                ui.label(RichText::new(EMPTY_CARD).color(theme::FG_DIM()));
                return;
            }
            for (index, component) in document.components.iter().enumerate() {
                if index > 0 {
                    ui.add_space(CARD_GAP);
                }
                show_component(ui, index, component, &mut view, enabled);
            }
        });
}

fn show_component(ui: &mut Ui, index: usize, component: &Component, view: &mut CardView<'_>, enabled: bool) {
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
        Component::List { selection, rows } => show_rows(ui, index, *selection, rows, view, enabled),
        Component::Buttons { items } => show_buttons(ui, items, view, enabled),
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

fn show_rows(
    ui: &mut Ui,
    index: usize,
    selection: Selection,
    rows: &[ListRow],
    view: &mut CardView<'_>,
    enabled: bool,
) {
    let selectable = selection == Selection::Single && enabled;
    for (row_index, row) in rows.iter().enumerate() {
        if row_index > 0 {
            ui.add_space(ROW_GAP);
        }
        if selectable {
            let selected = view.card.selected(index) == Some(row.id.as_str());
            if ui
                .selectable_label(selected, RichText::new(&row.title).color(theme::FG()))
                .clicked()
            {
                press_row(ui, index, row, view);
            }
        } else {
            ui.label(RichText::new(&row.title).color(theme::FG()));
        }
        if let Some(subtitle) = &row.subtitle {
            ui.label(RichText::new(subtitle).small().color(theme::FG_DIM()));
        }
    }
}

/// Selects the row the person pressed and records the click that goes with it.
fn press_row(ui: &mut Ui, index: usize, row: &ListRow, view: &mut CardView<'_>) {
    let (document, revision, now_millis) = (view.document, view.revision, view.now_millis);
    if view.card.select(index, &row.id, document) {
        view.card.press(&row.id, revision, document, now_millis);
        ui.ctx().request_repaint();
    }
}

fn show_buttons(ui: &mut Ui, items: &[Button], view: &mut CardView<'_>, enabled: bool) {
    ui.horizontal_wrapped(|ui| {
        for button in items {
            let pressed = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new(&button.label).color(tone_color(button.tone))),
                )
                .clicked();
            if pressed {
                let (document, revision, now_millis) = (view.document, view.revision, view.now_millis);
                view.card.press(&button.id, revision, document, now_millis);
                ui.ctx().request_repaint();
            }
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
