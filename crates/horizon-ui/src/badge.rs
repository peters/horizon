//! Shared rounded badge surface: a fill pass and a stroke pass with a shared
//! corner radius. Call sites keep their own radius, alpha table, and stroke
//! treatment in [`BadgeStyle`].

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

/// Surface styling for [`paint_badge`].
#[derive(Clone, Copy)]
pub(crate) struct BadgeStyle {
    pub radius: u8,
    pub fill: Color32,
    pub stroke: Stroke,
    pub stroke_kind: StrokeKind,
}

/// Paints a rounded badge surface (filled rect + stroked rect).
pub(crate) fn paint_badge(painter: &Painter, rect: Rect, style: BadgeStyle) {
    let radius = CornerRadius::same(style.radius);
    painter.rect_filled(rect, radius, style.fill);
    painter.rect_stroke(rect, radius, style.stroke, style.stroke_kind);
}
