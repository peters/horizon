//! The few line icons the cards need, drawn on a 24-unit grid so they share one
//! stroke weight and scale with the requested size.

use crate::theme;
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, vec2};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Send,
    Check,
    Bot,
    Note,
    Mic,
    Cross,
    Shield,
}

pub(super) fn paint(painter: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    let unit = size / 24.0;
    let origin = center - vec2(12.0, 12.0) * unit;
    let at = |x: f32, y: f32| origin + vec2(x, y) * unit;
    let stroke = Stroke::new((1.7 * unit).max(1.3), color);
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let outline = |min: (f32, f32), max: (f32, f32), radius: f32| {
        let rect = Rect::from_min_max(at(min.0, min.1), at(max.0, max.1));
        painter.rect_stroke(rect, CornerRadius::from(radius * unit), stroke, StrokeKind::Middle);
    };
    match icon {
        Icon::Send => {
            line(&[(6.0, 18.0), (18.0, 6.0)]);
            line(&[(9.0, 6.0), (18.0, 6.0), (18.0, 15.0)]);
        }
        Icon::Check => line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]),
        Icon::Bot => {
            outline((4.0, 8.0), (20.0, 20.0), 4.0);
            line(&[(12.0, 4.0), (12.0, 8.0)]);
            painter.circle_filled(at(9.0, 13.5), 1.4 * unit, color);
            painter.circle_filled(at(15.0, 13.5), 1.4 * unit, color);
        }
        Icon::Mic => {
            outline((9.0, 3.0), (15.0, 13.0), 3.0);
            line(&[
                (5.0, 11.0),
                (6.0, 14.0),
                (9.0, 16.5),
                (12.0, 17.5),
                (15.0, 16.5),
                (18.0, 14.0),
                (19.0, 11.0),
            ]);
            line(&[(12.0, 17.5), (12.0, 21.0)]);
        }
        Icon::Cross => {
            line(&[(6.5, 6.5), (17.5, 17.5)]);
            line(&[(17.5, 6.5), (6.5, 17.5)]);
        }
        Icon::Shield => {
            line(&[
                (12.0, 3.0),
                (20.0, 6.0),
                (20.0, 12.0),
                (12.0, 21.0),
                (4.0, 12.0),
                (4.0, 6.0),
                (12.0, 3.0),
            ]);
        }
        Icon::Note => {
            outline((5.0, 3.0), (19.0, 21.0), 2.5);
            line(&[(8.5, 9.0), (15.5, 9.0)]);
            line(&[(8.5, 13.0), (15.5, 13.0)]);
            line(&[(8.5, 17.0), (12.5, 17.0)]);
        }
    }
}

/// A rounded tile holding an icon, the lead of a card.
pub(super) fn tile(ui: &mut Ui, icon: Icon, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::from(size * 0.3), color.gamma_multiply(0.18));
    paint(ui.painter(), rect.center(), size * 0.56, icon, color);
}

/// The assistant's mark: a sun rising on the horizon, with three agents on an
/// arc above it. One centre, several agents around it, on a tile tinted with
/// the accent colour. Drawn from primitives, so it scales and follows the theme.
pub(super) fn paint_mark(painter: &Painter, rect: Rect) {
    let size = rect.width();
    let accent = theme::ACCENT();
    let tile = CornerRadius::from(size * 0.3);
    painter.rect_filled(rect, tile, theme::blend(theme::PANEL_BG(), accent, 0.2));
    painter.rect_stroke(
        rect,
        tile,
        Stroke::new(1.0, accent.gamma_multiply(0.4)),
        StrokeKind::Inside,
    );

    let at = |x: f32, y: f32| rect.min + vec2(x, y) * size;
    let horizon = 0.66;
    painter.line_segment(
        [at(0.2, horizon), at(0.8, horizon)],
        Stroke::new((size * 0.045).max(1.2), accent.gamma_multiply(0.7)),
    );

    // The sun: a half disc sitting on the horizon.
    let centre = at(0.5, horizon);
    let radius = size * 0.2;
    let sun: Vec<Pos2> = (0..=24_u8)
        .map(|step| {
            let angle = std::f32::consts::PI * (1.0 + f32::from(step) / 24.0);
            centre + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(Shape::convex_polygon(sun, accent, Stroke::NONE));

    // Three agents on an arc, each tied to the centre by a faint ray.
    let arc = size * 0.37;
    for degrees in [215.0_f32, 270.0, 325.0] {
        let angle = degrees.to_radians();
        let node = centre + vec2(angle.cos(), angle.sin()) * arc;
        painter.line_segment(
            [centre + vec2(angle.cos(), angle.sin()) * (radius + size * 0.03), node],
            Stroke::new(1.0, accent.gamma_multiply(0.3)),
        );
        painter.circle_filled(node, size * 0.055, theme::FG());
    }
}
