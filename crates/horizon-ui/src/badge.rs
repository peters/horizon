//! Shared painter primitive for semantic badge and pill backgrounds.

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

pub(crate) fn paint_badge_background(
    painter: &Painter,
    rect: Rect,
    corner_radius: CornerRadius,
    fill: Color32,
    stroke: Option<(Stroke, StrokeKind)>,
) {
    let (stroke, stroke_kind) = stroke.unwrap_or((Stroke::NONE, StrokeKind::Inside));
    let _ = painter.rect(rect, corner_radius, fill, stroke, stroke_kind);
}
