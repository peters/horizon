//! Painting recipes shared by the Dependencies panel and its dialogs. Values follow
//! docs/design/guidelines.md; text is a step larger because the panel is read zoomed out.

use std::sync::Arc;

use egui::{
    Align2, Color32, FontId, Galley, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::{text::single_line_label_job, theme};

/// The shared large-dialog frame.
pub(super) fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
        .corner_radius(16)
        .inner_margin(24)
}

/// A nested card such as the repository detail pane.
pub(super) fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(16)
}

/// A recessed well for prose or code the person reads but does not edit here.
pub(super) fn well() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::BG())
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 200)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
}

/// A field well around a control such as a combo box, so it reads as an input.
pub(super) fn field<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            // The well is the control's surface; the inner button only tints on interaction.
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            widgets.inactive.bg_stroke = Stroke::NONE;
            widgets.hovered.weak_bg_fill = theme::alpha(theme::PANEL_BG_ALT(), 160);
            widgets.hovered.bg_stroke = Stroke::NONE;
            widgets.open.weak_bg_fill = theme::alpha(theme::PANEL_BG_ALT(), 160);
            widgets.open.bg_stroke = Stroke::NONE;
            add(ui)
        })
        .inner
}

/// An upper-case section caption with an optional trailing note.
pub(super) fn caption(ui: &mut egui::Ui, text: &str, note: Option<&str>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(
            RichText::new(text.to_uppercase())
                .size(11.0)
                .strong()
                .extra_letter_spacing(0.6)
                .color(theme::FG_DIM()),
        );
        if let Some(note) = note {
            ui.label(RichText::new(note).size(12.0).color(theme::FG_SOFT()));
        }
    });
}

pub(super) fn chrome_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(13.0).color(theme::FG_SOFT()))
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)))
        .corner_radius(10)
        .min_size(vec2(0.0, 32.0))
}

pub(super) fn accent_text_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(13.0).color(theme::ACCENT()))
        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.08))
        .stroke(Stroke::new(
            1.0,
            theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.3),
        ))
        .corner_radius(8)
        .min_size(vec2(0.0, 30.0))
}

/// Dialog confirm: solid accent with the `BG` label, as "Start cloud".
pub(super) fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0).strong().color(theme::BG()))
        .fill(theme::ACCENT())
        .corner_radius(10)
        .min_size(vec2(120.0, 40.0))
}

pub(super) fn secondary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0))
        .corner_radius(10)
        .min_size(vec2(100.0, 40.0))
}

/// One segment of a segmented control, as the settings tab bar.
pub(super) fn segment(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let (fill, stroke, color) = if selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20),
            Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)),
            theme::FG(),
        )
    } else {
        (Color32::TRANSPARENT, Stroke::NONE, theme::FG_SOFT())
    };
    ui.add(
        egui::Button::selectable(selected, RichText::new(text).size(13.0).color(color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(8)
            .min_size(vec2(0.0, 30.0)),
    )
}

/// A read-only state tag: dot plus words in a translucent pill. Returns its rect.
pub(super) fn paint_pill(painter: &Painter, left_center: Pos2, label: &str, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), text_on_tint(color));
    let size = vec2(galley.size().x + 30.0, 22.0);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y / 2.0), size);
    painter.rect(
        rect,
        11,
        theme::alpha(color, 30),
        Stroke::new(1.0, theme::alpha(color, 70)),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, color);
    painter.galley(
        pos2(rect.left() + 19.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::FG(),
    );
    rect
}

pub(super) fn pill(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    let width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), color)
        .size()
        .x
        + 30.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    paint_pill(ui.painter(), rect.left_center(), label, color);
    response
}

/// Neutral state colors read as text; a pastel or deep state color keeps its hue.
fn text_on_tint(color: Color32) -> Color32 {
    if color == theme::FG_DIM() || color == theme::BORDER_STRONG() {
        theme::FG_SOFT()
    } else {
        color
    }
}

/// A small monospace tag, such as a package ecosystem. Returns its rect.
pub(super) fn paint_tag(painter: &Painter, left_center: Pos2, label: &str) -> Rect {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::monospace(11.5), theme::FG_SOFT());
    let size = vec2(galley.size().x + 14.0, 20.0);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y / 2.0), size);
    painter.rect(
        rect,
        6,
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)),
        StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::FG_SOFT(),
    );
    rect
}

pub(super) fn tag_width(painter: &Painter, label: &str) -> f32 {
    painter
        .layout_no_wrap(label.to_owned(), FontId::monospace(11.5), theme::FG_SOFT())
        .size()
        .x
        + 14.0
}

pub(super) fn tag(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(tag_width(ui.painter(), label), 20.0), Sense::hover());
    paint_tag(ui.painter(), rect.left_center(), label);
    response
}

/// Single-line text elided with an ellipsis at `max_width`.
pub(super) fn line(painter: &Painter, text: &str, font: &FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    painter.layout_job(single_line_label_job(text, font, color, max_width))
}

pub(super) fn paint_line(painter: &Painter, anchor: Pos2, align: Align2, galley: Arc<Galley>) -> Rect {
    let rect = align.anchor_size(anchor, galley.size());
    painter.galley(rect.min, galley, theme::FG());
    rect
}

pub(super) fn check_mark(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    let points = vec![
        center + vec2(-0.45, 0.0) * size,
        center + vec2(-0.12, 0.33) * size,
        center + vec2(0.48, -0.32) * size,
    ];
    painter.add(egui::Shape::line(points, stroke));
}

pub(super) fn cross_mark(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    let half = size * 0.36;
    painter.line_segment([center + vec2(-half, -half), center + vec2(half, half)], stroke);
    painter.line_segment([center + vec2(-half, half), center + vec2(half, -half)], stroke);
}

/// Three linked nodes: the product mark for dependency maintenance.
pub(super) fn dependency_badge(painter: &Painter, rect: Rect) {
    painter.rect(
        rect,
        12,
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20),
        Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.5)),
        StrokeKind::Inside,
    );
    let center = rect.center();
    let root = center + vec2(-8.0, 0.0);
    let leaves = [center + vec2(8.0, -8.0), center + vec2(8.0, 8.0)];
    let link = Stroke::new(1.6, theme::alpha(theme::ACCENT(), 190));
    for leaf in leaves {
        painter.line_segment([root, leaf], link);
        painter.circle_filled(leaf, 3.6, theme::ACCENT());
    }
    painter.circle_filled(root, 4.4, theme::ACCENT());
    painter.circle_filled(root, 1.6, theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20));
}

pub(super) fn magnifier(painter: &Painter, center: Pos2, color: Color32) {
    let lens = center - Vec2::splat(1.5);
    painter.circle_stroke(lens, 5.0, Stroke::new(1.5, color));
    painter.line_segment(
        [lens + Vec2::splat(3.8), lens + Vec2::splat(7.5)],
        Stroke::new(1.6, color),
    );
}

/// A vertical scroll area with a solid bar, as the cloud dialogs use.
pub(super) fn solid_scroll_area(ui: &mut egui::Ui) -> egui::ScrollArea {
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    egui::ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .auto_shrink([false, true])
}
