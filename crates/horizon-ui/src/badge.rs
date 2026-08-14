//! Shared painter primitive for semantic badge and pill backgrounds.

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind};

#[derive(Clone, Copy)]
pub(crate) struct BadgeStroke {
    stroke: Stroke,
    kind: StrokeKind,
}

impl BadgeStroke {
    pub(crate) const fn inside(stroke: Stroke) -> Self {
        Self {
            stroke,
            kind: StrokeKind::Inside,
        }
    }

    pub(crate) const fn outside(stroke: Stroke) -> Self {
        Self {
            stroke,
            kind: StrokeKind::Outside,
        }
    }
}

pub(crate) fn paint_badge_background(
    painter: &Painter,
    rect: Rect,
    corner_radius: CornerRadius,
    fill: Color32,
    stroke: Option<BadgeStroke>,
) {
    painter.rect_filled(rect, corner_radius, fill);
    if let Some(stroke) = stroke {
        painter.rect_stroke(rect, corner_radius, stroke.stroke, stroke.kind);
    }
}
