//! Small styled pieces the worker selector shares.
use crate::theme;
use egui::{Color32, Frame, RichText, Stroke, Ui};
use horizon_core::cloud_runtime::prices::Availability;

pub(super) fn heading(ui: &mut Ui, title: &str, detail: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(title).size(15.0).strong().color(theme::FG()));
        if !detail.is_empty() {
            ui.label(RichText::new(detail).size(12.5).color(theme::FG_DIM()));
        }
    });
}

pub(super) fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(theme::FG_SOFT()));
}

/// A small upper-case caption, as over each starting point.
pub(super) fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(10.5).strong().color(theme::FG_DIM()));
}

pub(super) fn pill(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(theme::alpha(color, 34))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).strong().color(color));
        });
}

/// Stock in words and colour; `None` is a worker not offered where the cloud may go.
pub(super) fn stock(stock: Option<Availability>) -> (&'static str, Color32) {
    match stock {
        Some(Availability::High | Availability::Medium) => ("In stock", theme::PALETTE_GREEN()),
        Some(Availability::Low) => ("Low stock", theme::PALETTE_YELLOW()),
        Some(Availability::None) => ("Out of stock", theme::PALETTE_RED()),
        None => ("Not offered here", theme::FG_DIM()),
    }
}

/// A framed status line, such as a running watch.
pub(super) fn status(ui: &mut Ui, color: Color32, title: &str, body: &str) {
    Frame::new()
        .fill(theme::alpha(color, 24))
        .stroke(Stroke::new(1.0, theme::alpha(color, 90)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(13.5).strong().color(color));
            ui.label(RichText::new(body).size(12.5).color(theme::FG_SOFT()));
        });
}

/// A row of a label and a right-aligned amount.
pub(super) fn line(ui: &mut Ui, label: &str, value: &str, strong: bool) {
    ui.horizontal(|ui| {
        let color = if strong { theme::FG() } else { theme::FG_SOFT() };
        let size = if strong { 14.0 } else { 13.0 };
        let mut label = RichText::new(label).size(size).color(color);
        let mut value = RichText::new(value).size(size).color(color);
        if strong {
            label = label.strong();
            value = value.strong();
        }
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(value);
        });
    });
}

/// A checkbox drawn to read clearly on the dialog's dark panels.
pub(super) fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> egui::Response {
    const BOX: f32 = 16.0;
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(13.5), theme::FG());
    let size = egui::vec2(BOX + 8.0 + text.size().x, BOX.max(text.size().y));
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let square = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - BOX / 2.0), egui::vec2(BOX, BOX));
    let enabled = ui.is_enabled();
    let accent = if enabled { theme::ACCENT() } else { theme::FG_DIM() };
    let painter = ui.painter();
    if *checked {
        painter.rect_filled(square, 4, accent);
        let points = [
            egui::pos2(square.left() + 3.5, square.center().y),
            egui::pos2(square.left() + 6.5, square.bottom() - 4.0),
            egui::pos2(square.right() - 3.5, square.top() + 4.0),
        ];
        painter.line(points.to_vec(), Stroke::new(2.0, theme::BG()));
    } else {
        let edge = if response.hovered() && enabled {
            theme::FG_SOFT()
        } else {
            theme::FG_DIM()
        };
        painter.rect_stroke(square, 4, Stroke::new(1.5, edge), egui::StrokeKind::Inside);
    }
    let color = if enabled { theme::FG() } else { theme::FG_DIM() };
    painter.galley(
        egui::pos2(square.right() + 8.0, rect.center().y - text.size().y / 2.0),
        text,
        color,
    );
    let checked = *checked;
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, label));
    response
}
