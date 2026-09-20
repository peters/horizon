//! Panel-local content zoom for browser and native device panels.
//!
//! Canvas zoom scales panel frames; it cannot scale what a panel shows. A
//! page reflows at its own zoom and a device desktop has a fixed remote
//! resolution that only the viewer can magnify, so both panels own a scale
//! of their own, changed by a dropdown or by a pinch/zoom-modifier wheel
//! over the content.

use egui::Ui;

pub(crate) const MIN_ZOOM: f32 = 0.25;
pub(crate) const MAX_ZOOM: f32 = 4.0;
/// Dropdown stops. Gestures still land on values between them.
const STOPS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
const DROPDOWN_WIDTH: f32 = 52.0;

/// A content scale clamped to the supported range. Default is unscaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PanelZoom(f32);

impl Default for PanelZoom {
    fn default() -> Self {
        Self::ONE
    }
}

impl PanelZoom {
    pub(crate) const ONE: Self = Self(1.0);

    /// Clamp to the supported range; a non-finite scale falls back to 100%.
    pub(crate) fn new(factor: f32) -> Self {
        if !factor.is_finite() {
            return Self::ONE;
        }
        Self(factor.clamp(MIN_ZOOM, MAX_ZOOM))
    }

    pub(crate) const fn factor(self) -> f32 {
        self.0
    }

    /// Apply a gesture multiplier from this scale, staying inside the range.
    pub(crate) fn scaled(self, delta: f32) -> Self {
        Self::new(self.0 * delta)
    }

    pub(crate) fn label(self) -> String {
        format!("{}%", (self.0 * 100.0).round())
    }
}

/// Percentage stops for a panel that always fills its body.
pub(crate) fn dropdown(ui: &mut Ui, id_salt: &'static str, zoom: &mut PanelZoom, interactive: bool) -> bool {
    let mut selection = Some(*zoom);
    let changed = show(ui, id_salt, &mut selection, false, interactive);
    if let Some(next) = selection {
        *zoom = next;
    }
    changed
}

/// Percentage stops plus `Fit`, which scales the content into the body.
pub(crate) fn dropdown_with_fit(
    ui: &mut Ui,
    id_salt: &'static str,
    selection: &mut Option<PanelZoom>,
    interactive: bool,
) -> bool {
    show(ui, id_salt, selection, true, interactive)
}

fn show(
    ui: &mut Ui,
    id_salt: &'static str,
    selection: &mut Option<PanelZoom>,
    allow_fit: bool,
    interactive: bool,
) -> bool {
    let before = *selection;
    let selected_text = selection.map_or_else(|| "Fit".to_owned(), PanelZoom::label);
    ui.add_enabled_ui(interactive, |ui| {
        egui::ComboBox::from_id_salt(id_salt)
            .width(DROPDOWN_WIDTH)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if allow_fit {
                    ui.selectable_value(selection, None, "Fit");
                }
                for stop in STOPS {
                    let zoom = PanelZoom::new(stop);
                    ui.selectable_value(selection, Some(zoom), zoom.label());
                }
            })
            .response
            .on_hover_text("Zoom this panel's content");
    });
    *selection != before
}

/// The pointer in the caller's layer coordinates. Panels are painted through
/// the canvas transform, so the global pointer does not match their rects.
pub(crate) fn local_pointer(ui: &Ui) -> Option<egui::Pos2> {
    let pointer = ui.input(|input| input.pointer.hover_pos())?;
    Some(
        ui.ctx()
            .layer_transform_from_global(ui.layer_id())
            .map_or(pointer, |from_global| from_global * pointer),
    )
}

/// Pinch, or wheel with the zoom modifier, over panel content. The canvas
/// leaves these gestures alone above panels that zoom their own content.
pub(crate) fn gesture_delta(ui: &Ui, hovered: bool) -> Option<f32> {
    if !hovered {
        return None;
    }
    let delta = ui.input(egui::InputState::zoom_delta);
    (delta.is_finite() && (delta - 1.0).abs() > f32::EPSILON).then_some(delta)
}

#[cfg(test)]
mod tests {
    use super::{MAX_ZOOM, MIN_ZOOM, PanelZoom, dropdown_with_fit, gesture_delta};
    use crate::test_egui::DiscardTextures;

    #[test]
    fn scales_are_clamped_and_labeled_as_whole_percentages() {
        assert_eq!(PanelZoom::default(), PanelZoom::ONE);
        assert!((PanelZoom::new(f32::NAN).factor() - 1.0).abs() <= f32::EPSILON);
        assert!((PanelZoom::new(99.0).factor() - MAX_ZOOM).abs() <= f32::EPSILON);
        assert!((PanelZoom::new(0.01).factor() - MIN_ZOOM).abs() <= f32::EPSILON);
        assert_eq!(PanelZoom::ONE.label(), "100%");
        assert_eq!(PanelZoom::new(0.666).label(), "67%");
    }

    #[test]
    fn gestures_are_ignored_off_content_and_without_a_zoom_event() {
        let ctx = egui::Context::default();
        let mut deltas = Vec::new();
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Zoom(1.25)],
                    ..Default::default()
                },
                |ui| deltas = vec![gesture_delta(ui, false), gesture_delta(ui, true)],
            )
            .discard_textures();
        assert_eq!(deltas[0], None);
        assert!(deltas[1].is_some_and(|delta| (delta - 1.25).abs() < 0.001));
        let mut idle = None;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| idle = gesture_delta(ui, true))
            .discard_textures();
        assert_eq!(idle, None);
    }

    #[test]
    fn the_dropdown_reports_only_an_actual_change() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let render = |events: Vec<egui::Event>| {
            let mut changed = None;
            let output = ctx
                .run_ui(
                    egui::RawInput {
                        events,
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let mut selection = Some(PanelZoom::ONE);
                        changed = dropdown_with_fit(ui, "zoom", &mut selection, true).then_some(selection);
                    },
                )
                .discard_textures();
            (changed, output)
        };
        let (unchanged, output) = render(Vec::new());
        assert_eq!(unchanged, None);
        let selected = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "100%" => Some(text.pos),
                _ => None,
            })
            .expect("the current zoom is displayed");
        for pressed in [true, false] {
            render(vec![
                egui::Event::PointerMoved(selected),
                egui::Event::PointerButton {
                    pos: selected,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        let (_, output) = render(Vec::new());
        let fit = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Fit" => Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            })
            .expect("the open dropdown lists Fit");
        let mut chosen = None;
        for pressed in [true, false] {
            let (changed, _) = render(vec![
                egui::Event::PointerMoved(fit),
                egui::Event::PointerButton {
                    pos: fit,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            chosen = chosen.or(changed);
        }
        assert_eq!(chosen, Some(None), "choosing Fit reports the new selection");
    }
}
