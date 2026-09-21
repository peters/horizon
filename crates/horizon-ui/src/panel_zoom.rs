//! Panel-local content zoom for browser and native device panels.
//!
//! Canvas zoom scales panel frames; it cannot scale what a panel shows. A
//! page reflows at its own zoom and a device desktop has a fixed remote
//! resolution that only the viewer can magnify, so both panels own a scale
//! of their own, changed by a dropdown or by a pinch/zoom-modifier wheel
//! over the content.

use egui::{Context, Id, Ui};

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

    pub(crate) fn label(self) -> String {
        format!("{}%", (self.0 * 100.0).round())
    }
}

/// The scale a gesture selects, starting from the scale on screen. `None`
/// keeps the current presentation: a fitted image can sit outside the
/// supported range, and clamping a gesture that pushes further out would move
/// the image the opposite way (a desktop fitted at 10% would grow on a
/// pinch out).
pub(crate) fn gesture_target(displayed: f32, delta: f32) -> Option<PanelZoom> {
    if !displayed.is_finite() || displayed <= 0.0 {
        // No usable scale on screen: treat the gesture as starting from 100%.
        return Some(PanelZoom::new(delta));
    }
    if (delta < 1.0 && displayed <= MIN_ZOOM) || (delta > 1.0 && displayed >= MAX_ZOOM) {
        return None;
    }
    Some(PanelZoom::new(displayed * delta))
}

/// Percentage stops for a panel that always fills its body. `displayed` is
/// the scale actually applied, which a panel may have had to cap.
pub(crate) fn dropdown(
    ui: &mut Ui,
    id_salt: &'static str,
    zoom: &mut PanelZoom,
    displayed: PanelZoom,
    interactive: bool,
) -> bool {
    let mut selection = Some(*zoom);
    let changed = show(ui, id_salt, &mut selection, Some(displayed), false, interactive);
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
    show(ui, id_salt, selection, None, true, interactive)
}

fn show(
    ui: &mut Ui,
    id_salt: &'static str,
    selection: &mut Option<PanelZoom>,
    displayed: Option<PanelZoom>,
    allow_fit: bool,
    interactive: bool,
) -> bool {
    let before = *selection;
    let selected_text = displayed
        .or(*selection)
        .map_or_else(|| "Fit".to_owned(), PanelZoom::label);
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

/// Idle gap that ends a gesture whose wheel events carry no touch phase,
/// matching the canvas scroll router.
pub(crate) const GESTURE_IDLE_SECONDS: f64 = 0.15;

/// Owner of the zoom gesture in progress, and when it was last seen. egui
/// keeps smoothing wheel zoom for several frames after the input stops,
/// and the pointer can leave the panel it started on, so one gesture must
/// keep one owner instead of splitting between a panel and the canvas.
#[derive(Clone, Copy)]
struct GestureLatch {
    /// The panel layer that owns it, or `None` for the canvas.
    owner: Option<Id>,
    last_seen: f64,
    kind: GestureKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GestureKind {
    NativePinch,
    Wheel,
}

impl GestureKind {
    fn from_input(input: &egui::InputState) -> Self {
        if input.multi_touch().is_some()
            || input.raw.events.iter().any(|event| {
                matches!(event, egui::Event::Zoom(delta) if delta.is_finite() && (delta - 1.0).abs() > f32::EPSILON)
            })
        {
            Self::NativePinch
        } else {
            // egui smooths wheel zoom only; native zoom resets each pass.
            Self::Wheel
        }
    }
}

pub(crate) fn is_native_pinch(ctx: &Context) -> bool {
    ctx.input(|input| GestureKind::from_input(input) == GestureKind::NativePinch)
}

fn latch_id(ctx: &Context) -> Id {
    Id::new(("panel_zoom_gesture", ctx.viewport_id()))
}

/// Board and fullscreen content use different layers and hit-test geometry.
/// A rendering-mode transition starts routing afresh in the new view.
pub(crate) fn synchronize_fullscreen(ctx: &Context, fullscreen_panel: Option<Id>) {
    let latch = latch_id(ctx);
    let mode = Id::new(("panel_zoom_fullscreen", ctx.viewport_id()));
    ctx.data_mut(|data| {
        let previous = data.get_temp::<Option<Id>>(mode).flatten();
        if previous != fullscreen_panel {
            data.remove::<GestureLatch>(latch);
        }
        data.insert_temp(mode, fullscreen_panel);
    });
}

fn fresh_latch(ctx: &Context, now: f64) -> Option<GestureLatch> {
    ctx.data(|data| data.get_temp::<GestureLatch>(latch_id(ctx)))
        .filter(|latch| now - latch.last_seen <= GESTURE_IDLE_SECONDS)
}

/// Reserve new gestures for foreground controls such as menus. Known panel
/// layers are excluded because their previous-frame hit geometry may be stale.
pub(crate) fn blocking_layer(ctx: &Context, mut panel_layers: impl Iterator<Item = Id>) -> Option<Id> {
    let pointer = ctx.input(|input| input.pointer.hover_pos())?;
    let layer = ctx.layer_id_at(pointer)?;
    (layer.order >= egui::Order::Foreground && !panel_layers.any(|id| id == layer.id)).then_some(layer.id)
}

/// A host-painted content menu shares its panel's layer. Give new gestures
/// a separate owner while it is open so neither content nor canvas moves.
pub(crate) fn content_owner(layer: Id, menu_open: bool) -> Id {
    if menu_open {
        layer.with("content_menu_zoom")
    } else {
        layer
    }
}

/// Latch who owns the gesture in progress. `candidate` is what the pointer
/// says right now; the first answer wins until idle or an input-kind change.
pub(crate) fn gesture_owner(ctx: &Context, candidate: Option<Id>) -> Option<Id> {
    let (active, now, kind) = ctx.input(|input| {
        (
            (input.zoom_delta() - 1.0).abs() > f32::EPSILON,
            input.time,
            GestureKind::from_input(input),
        )
    });
    if !active {
        return candidate;
    }
    let owner = fresh_latch(ctx, now)
        .filter(|latch| latch.kind == kind)
        .map_or(candidate, |latch| latch.owner);
    let id = latch_id(ctx);
    ctx.data_mut(|data| {
        data.insert_temp(
            id,
            GestureLatch {
                owner,
                last_seen: now,
                kind,
            },
        );
    });
    owner
}

/// Whether this panel owns the gesture. While one is latched the owner keeps
/// it; otherwise the pointer decides, over the panel body — the whole area a
/// panel widget is given, and exactly the rectangle the canvas leaves alone.
pub(crate) fn owns_gesture(ui: &Ui) -> bool {
    let now = ui.input(|input| input.time);
    fresh_latch(ui.ctx(), now).map_or_else(
        || ui.rect_contains_pointer(ui.max_rect()),
        |latch| latch.owner == Some(ui.layer_id().id),
    )
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
    use super::{MAX_ZOOM, MIN_ZOOM, PanelZoom, dropdown_with_fit, gesture_delta, gesture_target};
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
    fn a_gesture_never_moves_the_image_against_its_own_direction() {
        // Inside the range a gesture simply scales.
        assert_eq!(gesture_target(1.0, 1.25), Some(PanelZoom::new(1.25)));
        assert_eq!(gesture_target(1.0, 0.5), Some(PanelZoom::new(0.5)));
        // A desktop fitted below the range stays fitted on a pinch out, and
        // grows to the nearest supported scale on a pinch in.
        assert_eq!(gesture_target(0.1, 0.5), None);
        assert_eq!(gesture_target(0.1, 1.5), Some(PanelZoom::new(MIN_ZOOM)));
        // A tiny desktop fitted above the range behaves symmetrically.
        assert_eq!(gesture_target(8.0, 1.5), None);
        assert_eq!(gesture_target(8.0, 0.5), Some(PanelZoom::new(MAX_ZOOM)));
        // The ends of the range absorb gestures that push past them.
        assert_eq!(gesture_target(MIN_ZOOM, 0.5), None);
        assert_eq!(gesture_target(MAX_ZOOM, 1.5), None);
        // A degenerate layout still accepts the gesture from 100%.
        assert_eq!(gesture_target(0.0, 1.25), Some(PanelZoom::new(1.25)));
    }

    #[test]
    fn one_gesture_keeps_one_owner_until_it_goes_idle() {
        use super::gesture_owner;
        let ctx = egui::Context::default();
        let panel = egui::Id::new("panel-a");
        let other = egui::Id::new("panel-b");
        let owner_at = |time: f64, zooming: bool, candidate: Option<egui::Id>| {
            let mut owner = None;
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        time: Some(time),
                        events: if zooming {
                            vec![egui::Event::Zoom(1.1)]
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    },
                    |ui| owner = gesture_owner(ui.ctx(), candidate),
                )
                .discard_textures();
            owner
        };
        // The pointer's panel claims the gesture, and keeps it while the
        // smoothed delta continues even after the pointer moves elsewhere.
        assert_eq!(owner_at(1.0, true, Some(panel)), Some(panel));
        assert_eq!(owner_at(1.05, true, Some(other)), Some(panel));
        assert_eq!(owner_at(1.1, true, None), Some(panel));
        // A gesture that starts on the canvas keeps the canvas the same way.
        assert_eq!(owner_at(5.0, true, None), None);
        assert_eq!(owner_at(5.05, true, Some(panel)), None);
        // Once the gesture goes idle the next one is claimed afresh.
        assert_eq!(owner_at(9.0, true, Some(other)), Some(other));
        // With no gesture in flight the pointer's own answer is returned.
        assert_eq!(owner_at(9.02, false, Some(panel)), Some(panel));
    }

    #[test]
    fn wheel_and_native_pinch_claim_separate_gestures() {
        let ctx = egui::Context::default();
        let panel = egui::Id::new("fixed-browser");
        let other = egui::Id::new("another-panel");
        let wheel = || {
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 4.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::CTRL,
            }]
        };
        let mut mixed = wheel();
        mixed.push(egui::Event::Zoom(1.25));
        for (index, (events, candidate, expected)) in [
            (wheel(), Some(panel), Some(panel)),
            (Vec::new(), Some(other), Some(panel)),
            (vec![egui::Event::Zoom(1.25)], None, None),
            (Vec::new(), Some(panel), Some(panel)),
            (mixed, None, None),
            (wheel(), Some(panel), Some(panel)),
        ]
        .into_iter()
        .enumerate()
        {
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        time: Some(1.0 + f64::from(u32::try_from(index).expect("small index")) * 0.02),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        assert!(ui.input(|input| (input.zoom_delta() - 1.0).abs() > f32::EPSILON));
                        assert_eq!(super::gesture_owner(ui.ctx(), candidate), expected, "frame {index}");
                    },
                )
                .discard_textures();
        }
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
