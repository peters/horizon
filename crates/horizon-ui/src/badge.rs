//! Shared painter primitive for rounded badge backgrounds.

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

pub(crate) fn paint_badge_background(
    painter: &Painter,
    rect: Rect,
    corner_radius: CornerRadius,
    fill: Color32,
    stroke: Stroke,
    stroke_kind: StrokeKind,
) {
    painter.rect_filled(rect, corner_radius, fill);
    if !stroke.is_empty() {
        painter.rect_stroke(rect, corner_radius, stroke, stroke_kind);
    }
}
