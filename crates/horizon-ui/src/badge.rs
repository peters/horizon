//! Shared painter primitive for semantic badge and pill backgrounds.

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

pub(crate) fn paint_badge_background(
    painter: &Painter,
    rect: Rect,
    corner_radius: CornerRadius,
    fill: Color32,
    stroke: Option<(Stroke, StrokeKind)>,
) {
    painter.rect_filled(rect, corner_radius, fill);
    if let Some((stroke, stroke_kind)) = stroke {
        painter.rect_stroke(rect, corner_radius, stroke, stroke_kind);
    }
}
