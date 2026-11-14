//! The strip under a terminal grid that says why the panel has no shell.
//!
//! Two panels keep their last screen and take no keys: one whose link is lost and
//! one whose program ended. Both say it in the same strip, so a person reads the
//! same shape of line and the same kind of control in either of them.

use egui::{Align, Layout, Rect, Ui, UiBuilder, Vec2};

use crate::theme;

/// The height of the strip at the bottom of the grid, in points.
const HEIGHT: f32 = 26.0;
/// The padding inside the strip.
const PADDING: Vec2 = Vec2::new(10.0, 4.0);
/// The size of the text of the line and of its controls.
const TEXT_SIZE: f32 = 11.5;

/// Paints the strip at the bottom of `body` and answers the rectangle inside it,
/// or `None` while the strip is off the screen.
pub(super) fn paint(ui: &Ui, body: Rect) -> Option<Rect> {
    let strip = Rect::from_min_max(egui::pos2(body.min.x, (body.max.y - HEIGHT).max(body.min.y)), body.max);
    if !ui.is_rect_visible(strip) {
        return None;
    }
    ui.painter().rect_filled(strip, 0.0, theme::PANEL_BG_ALT());
    Some(strip.shrink2(PADDING))
}

/// Runs `contents` left to right inside the strip rectangle `strip`.
pub(super) fn row<R>(ui: &mut Ui, strip: Rect, contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(strip)
            .layout(Layout::left_to_right(Align::Center)),
        contents,
    )
    .inner
}

/// Shows the sentence of the strip. A narrow panel shortens it instead of
/// pushing the controls out of the strip.
pub(super) fn sentence(ui: &mut Ui, text: &str) {
    let text = crate::zoom_text::widget_text(ui.painter(), text, font(), theme::FG_SOFT(), Some(ui.available_width()));
    ui.add(egui::Label::new(text).truncate());
}

/// The font of the line and of the controls of the strip.
fn font() -> egui::FontId {
    egui::FontId::proportional(TEXT_SIZE)
}

/// Lays the sentence at the left of the strip and its controls at the right,
/// where the controls come from the edge in the order that `controls` adds them.
pub(super) fn sides<Ret>(ui: &mut Ui, text: &str, controls: impl FnOnce(&mut Ui) -> Ret) -> Ret {
    let ((), pressed) = egui::Sides::new()
        .shrink_left()
        .truncate()
        .show(ui, |ui| sentence(ui, text), controls);
    pressed
}

/// Shows one control at the right side of the strip and answers whether the
/// person pressed it in this pass. The caller places the controls in one
/// right-to-left layout, so the first control that it adds sits at the edge.
pub(super) fn control(ui: &mut Ui, label: &str, interactive: bool) -> bool {
    let text = crate::zoom_text::widget_text(ui.painter(), label, font(), theme::FG_SOFT(), None);
    let button = egui::Button::new(text)
        .fill(theme::PANEL_BG())
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE()))
        .corner_radius(10);
    ui.add_enabled(interactive, button).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom_text::measure_canvas_quads;

    /// The sentence and the controls of the strip are made for the screen size of
    /// the canvas zoom. A stock label and a stock button would be a picture of
    /// zoom 1 that the layer scales, and `Start again` would blur at a zoom of 67
    /// percent.
    #[test]
    fn the_strip_of_a_panel_is_sharp_at_every_zoom() {
        for display_scale in [1.0_f32, 2.0] {
            for zoom in [0.67_f32, 1.0, 1.5] {
                let (quads, worst_phase, worst_stretch) = measure_canvas_quads(display_scale, zoom, |ui, _| {
                    let strip = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(420.0, HEIGHT));
                    row(ui, strip, |ui| {
                        sides(ui, "The program ended", |ui| {
                            let _ = control(ui, "Start again", true);
                            let _ = control(ui, "Close", true);
                        });
                    });
                });
                assert!(quads > 20, "the strip draws its sentence and its controls: {quads}");
                assert!(
                    worst_phase <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph corner sits {worst_phase} device pixels from a whole pixel"
                );
                assert!(
                    worst_stretch <= 0.01,
                    "at zoom {zoom} and display scale {display_scale} a glyph picture is stretched by {worst_stretch} device pixels"
                );
            }
        }
    }
}
