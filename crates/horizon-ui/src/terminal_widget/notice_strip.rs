//! The strip under a terminal grid that says why the panel has no shell.
//!
//! Two panels keep their last screen and take no keys: one whose link is lost and
//! one whose program ended. Both say it in the same strip, so a person reads the
//! same shape of line and the same kind of control in either of them. A follow
//! panel whose stream ended has no control, so its line takes the strip alone.

use egui::{Align, Layout, Rect, Ui, UiBuilder, Vec2};

use crate::theme;

/// The height of the strip at the bottom of the grid, in points, with one row.
pub(super) const HEIGHT: f32 = 26.0;
/// The padding inside the strip.
const PADDING: Vec2 = Vec2::new(10.0, 4.0);
/// The size of the text of the line and of its controls.
const TEXT_SIZE: f32 = 11.5;
/// The most rows that the line of an ended stream takes. A panel narrower than
/// four rows of the line needs keeps its sentence whole as hover text.
pub(super) const MAX_ROWS: usize = 4;

/// Paints the strip at the bottom of `body`, `height` points high, and answers the
/// rectangle inside it, or `None` while the strip is off the screen.
pub(super) fn paint(ui: &Ui, body: Rect, height: f32) -> Option<Rect> {
    let strip = Rect::from_min_max(egui::pos2(body.min.x, (body.max.y - height).max(body.min.y)), body.max);
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

/// Shows the line of a stream that ended under the grid of `body`: in the rows
/// that its words need and at most [`MAX_ROWS`] of them, with the whole line
/// while the pointer rests on a line that is cut. The strip is as tall as those
/// rows, so it covers the screen no more than the line needs.
pub(super) fn ended_line(ui: &Ui, body: Rect, text: &str) {
    let width = (body.width() - 2.0 * PADDING.x).max(1.0);
    let galley = crate::zoom_text::layout_job(
        ui.painter(),
        crate::text::wrapped_rows_job(text, &font(), theme::FG_SOFT(), width, MAX_ROWS),
    );
    let height = (galley.size().y + 2.0 * PADDING.y).max(HEIGHT);
    let Some(strip) = paint(ui, body, height) else {
        return;
    };
    let elided = galley.elided;
    let painted = Rect::from_min_size(strip.min, galley.size());
    ui.painter().galley(strip.min, galley, theme::FG_SOFT());
    if elided {
        // The rows that do not fit are the whole line while the pointer rests on it.
        ui.interact(painted, ui.scope_id().with("ended_line"), egui::Sense::hover())
            .on_hover_text(text);
    }
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
    use crate::test_egui::DiscardTextures;
    use crate::zoom_text::measure_canvas_quads;

    /// One pass of the strip of an ended stream at `body`, and the galleys that it
    /// drew. `pointer` moves the pointer in that pass and rests it there after.
    fn drawn(
        ctx: &egui::Context,
        body: Rect,
        pointer: Option<egui::Pos2>,
        time: f64,
    ) -> Vec<std::sync::Arc<egui::Galley>> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
            time: Some(time),
            events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
            ..egui::RawInput::default()
        };
        let text = "The stream of this terminal ended: the other computer closed this terminal or \
                    shares it no longer. Text typed here is not sent.";
        let output = ctx.run_ui(input, |ui| ended_line(ui, body, text)).discard_textures();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(drawn) => Some(drawn.galley.clone()),
                _ => None,
            })
            .collect()
    }

    /// The line of an ended stream is laid out in the rows that its words need. A
    /// panel that is narrower than four rows of it keeps those four rows and offers
    /// the whole line while the pointer rests on them, so no panel hides the reason.
    #[test]
    fn a_line_that_does_not_fit_keeps_its_whole_text_as_hover_text() {
        let body = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(130.0, 200.0));
        let pointer = egui::pos2(60.0, 205.0);
        let ctx = egui::Context::default();
        let mut galleys = drawn(&ctx, body, Some(pointer), 0.0);
        for time in [0.6_f64, 0.7] {
            galleys = drawn(&ctx, body, None, time);
        }
        let strip = galleys.first().expect("the strip draws its line");
        assert!(
            strip.elided,
            "the line is cut after the rows that fit: {:?}",
            strip.rows.len()
        );
        assert_eq!(strip.rows.len(), MAX_ROWS, "the line takes the four rows at most");
        assert!(
            galleys
                .iter()
                .filter(|galley| !galley.elided)
                .any(|galley| galley.text() == strip.text()),
            "the whole line shows while the pointer rests on the rows: {}",
            galleys.len()
        );
    }

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
