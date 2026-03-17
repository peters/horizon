use std::f32::consts::TAU;
use std::time::Duration;

use egui::{Color32, Id, Pos2, RichText, Stroke, Vec2};

use crate::theme;

const SPINNER_RADIUS: f32 = 12.0;
const STROKE_WIDTH: f32 = 2.0;
const ARC_LENGTH: f32 = TAU * 0.28;
const ROTATION_PERIOD: f64 = 1.2;
const LABEL_SPACING: f32 = 10.0;

/// Paints a centered loading spinner with an optional label below it.
///
/// The spinner is a rotating arc that animates continuously. Pass a stable
/// `Id` so the animation phase persists across frames.
pub fn show(ui: &mut egui::Ui, id: Id, label: Option<&str>) {
    show_colored(ui, id, label, theme::ACCENT);
}

/// Like [`show`] but lets the caller pick the arc color.
pub fn show_colored(ui: &mut egui::Ui, id: Id, label: Option<&str>, color: Color32) {
    show_inner(ui, id, label, None, color);
}

/// Spinner with a primary label and a smaller detail line below it.
pub fn show_with_detail(ui: &mut egui::Ui, id: Id, label: &str, detail: &str) {
    show_inner(ui, id, Some(label), Some(detail), theme::ACCENT);
}

fn show_inner(ui: &mut egui::Ui, id: Id, label: Option<&str>, detail: Option<&str>, color: Color32) {
    ui.ctx().request_repaint_after(Duration::from_millis(32));

    let label_height = label.map_or(0.0, |_| LABEL_SPACING + 14.0);
    let detail_height = detail.map_or(0.0, |_| 4.0 + 12.0);
    let total_height = SPINNER_RADIUS * 2.0 + label_height + detail_height;

    ui.vertical_centered(|ui| {
        let available = ui.available_height();
        if available > total_height {
            ui.add_space((available - total_height) * 0.38);
        }

        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(SPINNER_RADIUS * 2.0, SPINNER_RADIUS * 2.0),
            egui::Sense::hover(),
        );

        let center = rect.center();
        paint_arc(ui, id, center, color);

        if let Some(text) = label {
            ui.add_space(LABEL_SPACING);
            ui.label(RichText::new(text).size(12.0).color(theme::FG_DIM));
        }

        if let Some(text) = detail {
            ui.add_space(4.0);
            ui.label(RichText::new(text).size(11.0).color(theme::FG_DIM));
        }
    });
}

fn paint_arc(ui: &egui::Ui, id: Id, center: Pos2, color: Color32) {
    let time = ui.input(|i| i.time);
    let angle = ((time % ROTATION_PERIOD) / ROTATION_PERIOD) as f32 * TAU;

    // Fade the arc: full opacity at the leading edge, fading toward the tail.
    let segments = 32_u32;
    let stroke = Stroke::new(STROKE_WIDTH, color);

    let points: Vec<Pos2> = (0..=segments)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 / segments as f32;
            let a = angle + t * ARC_LENGTH;
            Pos2::new(center.x + a.cos() * SPINNER_RADIUS, center.y + a.sin() * SPINNER_RADIUS)
        })
        .collect();

    let painter = ui.painter_at(egui::Rect::from_center_size(
        center,
        Vec2::splat(SPINNER_RADIUS * 2.0 + STROKE_WIDTH * 2.0),
    ));

    // Draw fading segments from tail (transparent) to head (opaque).
    for i in 0..segments {
        #[allow(clippy::cast_precision_loss)]
        let alpha_frac = (i + 1) as f32 / segments as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let a = (alpha_frac * f32::from(color.a())).round().clamp(0.0, 255.0) as u8;
        let seg_color = theme::alpha(color, a);
        painter.line_segment(
            [points[i as usize], points[(i + 1) as usize]],
            Stroke::new(stroke.width, seg_color),
        );
    }

    // Keep the animation ID alive so egui knows the widget is active.
    let _ = id;
}
