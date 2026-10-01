//! The shared look of an activity card: a tinted frame, an icon tile with a
//! title, and a status pill. Cards fill the body themselves.

use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2,
};

use super::icons::{self, Icon};
use crate::theme;

#[derive(Clone, Copy)]
pub(super) enum Tone {
    /// Waiting on the person.
    Attention,
    /// Finished and healthy.
    Good,
    /// Plain information.
    Neutral,
}

impl Tone {
    pub(super) fn accent(self) -> Color32 {
        match self {
            Self::Attention => theme::ACCENT(),
            Self::Good => theme::PALETTE_GREEN(),
            Self::Neutral => theme::FG_SOFT(),
        }
    }

    fn fill_and_stroke(self) -> (Color32, Color32) {
        match self {
            Self::Neutral => (theme::BG_ELEVATED(), theme::BORDER_SUBTLE()),
            tone => (
                theme::blend(theme::PANEL_BG(), tone.accent(), 0.10),
                theme::blend(theme::PANEL_BG(), tone.accent(), 0.35),
            ),
        }
    }
}

/// Frames a card and returns what `add_body` returns.
pub(super) fn card<R>(ui: &mut Ui, tone: Tone, add_body: impl FnOnce(&mut Ui) -> R) -> R {
    let (fill, stroke) = tone.fill_and_stroke();
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_body(ui)
        })
        .inner
}

/// Icon tile, title and subtitle on the left, an optional status pill on the right.
pub(super) fn header(ui: &mut Ui, icon: Icon, tone: Tone, title: &str, subtitle: &str, pill: Option<(&str, Color32)>) {
    ui.horizontal(|ui| {
        icons::tile(ui, icon, tone.accent(), 30.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(13.5).strong().color(theme::FG()));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(11.5).color(theme::FG_DIM()));
            }
        });
        if let Some((text, color)) = pill {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| status_pill(ui, text, color));
        }
    });
}

/// A pill sized from its measured text, so it never stretches in a right-to-left row.
fn status_pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let size = galley.size() + vec2(18.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(99),
        color.gamma_multiply(0.14),
        Stroke::new(1.0, color.gamma_multiply(0.4)),
        StrokeKind::Inside,
    );
    painter.galley(rect.left_top() + vec2(9.0, 4.0), galley, color);
}

/// Monospace text for messages exchanged with an agent.
pub(super) fn mono(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).font(FontId::monospace(11.5)).color(color)
}

/// A filled button for the primary choice of a card.
pub(super) fn primary(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(12.5)
                .strong()
                .color(Color32::from_rgb(7, 16, 31)),
        )
        .fill(theme::ACCENT())
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 30.0)),
    )
}

/// A quiet button for the secondary choice of a card.
pub(super) fn ghost(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(theme::FG_SOFT()))
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 30.0)),
    )
}
