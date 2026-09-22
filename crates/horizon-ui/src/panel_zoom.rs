//! Panel-local content zoom for browser and native device panels.
//!
//! Canvas zoom scales panel frames; it cannot scale what a panel shows. A
//! page reflows at its own zoom and a device desktop has a fixed remote
//! resolution that only the viewer can magnify, so both panels own a scale
//! of their own, changed by a dropdown or by a pinch/zoom-modifier wheel
//! over the content.

use std::sync::Arc;

use egui::{Context, Id, RichText, Ui};

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
    let selected_text = dropdown_label(ui, displayed.or(*selection));
    ui.add_enabled_ui(interactive, |ui| {
        egui::ComboBox::from_id_salt(id_salt)
            .width(DROPDOWN_WIDTH)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if allow_fit {
                    ui.selectable_value(selection, None, dropdown_label(ui, None));
                }
                for stop in STOPS {
                    let zoom = PanelZoom::new(stop);
                    ui.selectable_value(selection, Some(zoom), dropdown_label(ui, Some(zoom)));
                }
            })
            .response
            .on_hover_text("Zoom this panel's content");
    });
    *selection != before
}

fn dropdown_label(ui: &Ui, zoom: Option<PanelZoom>) -> Arc<RichText> {
    let font = egui::FontSelection::default().resolve_with_fallback(ui.style(), egui::TextStyle::Button.into());
    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font)) + ui.spacing().extra_text_line_spacing;
    cached_label(ui.ctx(), zoom, line_height)
}

fn cached_label(ctx: &Context, zoom: Option<PanelZoom>, line_height: f32) -> Arc<RichText> {
    // Whole percentages need at most 376 labels plus Fit per font line height.
    // RichText keeps egui's ComboBox clone cheap while layout remains theme-aware.
    let percentage = zoom.map(|zoom| (zoom.factor() * 100.0).round().to_bits());
    let id = Id::new(("panel_zoom_label", percentage, line_height.to_bits()));
    ctx.data_mut(|data| {
        Arc::clone(data.get_temp_mut_or_insert_with(id, || {
            Arc::new(
                RichText::new(zoom.map_or_else(|| "Fit".to_owned(), PanelZoom::label)).line_height(Some(line_height)),
            )
        }))
    })
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
    /// The wheel gesture it belongs to; see [`gesture_generation`].
    generation: u64,
}

/// Wheel phases seen so far, in the order they arrived.
#[derive(Clone, Copy, Default)]
struct WheelPhases {
    /// The frame `sample` was computed for.
    frame: Option<u64>,
    /// The gesture this frame's zoom sample belongs to.
    sample: u64,
    /// The gesture current once all of this frame's phases are applied.
    settled: u64,
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
pub(crate) fn synchronize_fullscreen(ctx: &Context, fullscreen_panel: Option<Id>) -> bool {
    let latch = latch_id(ctx);
    let mode = Id::new(("panel_zoom_fullscreen", ctx.viewport_id()));
    let deferred = deferred_canvas_id(ctx);
    ctx.data_mut(|data| {
        let previous = data.get_temp::<Option<Id>>(mode).flatten();
        if previous != fullscreen_panel {
            data.remove::<GestureLatch>(latch);
            data.remove::<DeferredCanvasZoom>(deferred);
        }
        data.insert_temp(mode, fullscreen_panel);
        previous != fullscreen_panel
    })
}

fn fresh_latch(ctx: &Context, now: f64) -> Option<GestureLatch> {
    let generation = gesture_generation(ctx);
    ctx.data(|data| data.get_temp::<GestureLatch>(latch_id(ctx)))
        .filter(|latch| latch.generation == generation && now - latch.last_seen <= GESTURE_IDLE_SECONDS)
}

/// The wheel gesture this frame's zoom sample belongs to. Explicit
/// `Start`/`End`/`Cancel` phases bound a gesture exactly, so a new gesture
/// never inherits the previous one's owner or anchor however soon it starts;
/// the idle gap remains the boundary for phase-less wheels and native pinch.
/// Phases are applied in arrival order, so a frame that ends one gesture and
/// starts another samples under the new one, while a final move batched with
/// its own end still belongs to the gesture it ends.
pub(crate) fn gesture_generation(ctx: &Context) -> u64 {
    let id = Id::new(("panel_zoom_wheel_phases", ctx.viewport_id()));
    let frame = ctx.cumulative_frame_nr();
    let phases = ctx.data(|data| data.get_temp::<WheelPhases>(id)).unwrap_or_default();
    if phases.frame == Some(frame) {
        return phases.sample;
    }
    let mut settled = phases.settled;
    let mut sample = None;
    ctx.input(|input| {
        for event in &input.raw.events {
            if let egui::Event::MouseWheel { phase, delta, .. } = event {
                match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::End | egui::TouchPhase::Cancel => settled += 1,
                    egui::TouchPhase::Move if *delta != egui::Vec2::ZERO => sample = Some(settled),
                    egui::TouchPhase::Move => {}
                }
            }
        }
    });
    let sample = sample.unwrap_or(settled);
    ctx.data_mut(|data| {
        data.insert_temp(
            id,
            WheelPhases {
                frame: Some(frame),
                sample,
                settled,
            },
        );
    });
    sample
}

/// Reserve foreground controls using retained geometry, except app dialogs
/// whose current interactive coverage is already resolved by their owner.
pub(crate) fn blocking_layer(
    ctx: &Context,
    mut panel_layers: impl Iterator<Item = Id>,
    ignore_layer: impl Fn(Id) -> bool,
) -> Option<Id> {
    let pointer = ctx.input(|input| input.pointer.hover_pos())?;
    let mut layer = ctx.layer_id_at(pointer)?;
    if ignore_layer(layer.id) {
        // A retired backdrop may cover an unrelated live menu. Search below it,
        // stopping at the first real hit (including a panel covering a menu).
        let layers = ctx.memory(|memory| {
            memory
                .layer_ids()
                .filter(|layer| memory.areas().is_visible(layer))
                .collect::<Vec<_>>()
        });
        layer = layers.into_iter().rev().find_map(|layer| {
            if ignore_layer(layer.id) {
                return None;
            }
            let area = egui::AreaState::load(ctx, layer.id)?;
            let rect = area.rect();
            let rect = ctx
                .layer_transform_to_global(layer)
                .map_or(rect, |transform| transform * rect);
            (area.interactable && rect.contains(pointer)).then_some(layer)
        })?;
    }
    (layer.order >= egui::Order::Foreground && !panel_layers.any(|id| id == layer.id)).then_some(layer.id)
}

/// A host-painted menu needs its current body layout before routing can
/// distinguish menu, content and canvas. Reserve its panel until that layout.
pub(crate) fn content_owner(layer: Id, menu_open: bool) -> Id {
    if menu_open {
        layer.with("deferred_content_zoom")
    } else {
        layer
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DeferredCanvasZoom {
    pub(crate) anchor: egui::Pos2,
    pub(crate) delta: f32,
}

fn deferred_canvas_id(ctx: &Context) -> Id {
    Id::new(("deferred_browser_canvas_zoom", ctx.viewport_id()))
}

/// Resolve only a newly reserved gesture. An owner already chosen on an
/// earlier sample must survive pointer movement and a menu opening/closing.
/// Returns true when this sample starts outside the menu and should dismiss it.
pub(crate) fn resolve_content_owner(ui: &Ui, menu_contains_pointer: bool, canvas_fallback: bool) -> bool {
    let ctx = ui.ctx();
    if gesture_delta(ui, true).is_none() {
        return false;
    }
    let now = ui.input(|input| input.time);
    let Some(mut latch) = fresh_latch(ctx, now) else {
        return false;
    };
    let layer = ui.layer_id().id;
    if latch.owner != Some(content_owner(layer, true)) {
        return false;
    }
    latch.owner = if menu_contains_pointer {
        Some(layer.with("content_menu_zoom"))
    } else if canvas_fallback {
        None
    } else {
        Some(layer)
    };
    let latch_key = latch_id(ctx);
    ctx.data_mut(|data| data.insert_temp(latch_key, latch));
    if !menu_contains_pointer && canvas_fallback {
        let (anchor, delta) = ui.input(|input| (input.pointer.hover_pos(), input.zoom_delta()));
        if let Some(anchor) = anchor {
            let deferred_key = deferred_canvas_id(ctx);
            ctx.data_mut(|data| data.insert_temp(deferred_key, DeferredCanvasZoom { anchor, delta }));
        }
    }
    !menu_contains_pointer
}

pub(crate) fn take_deferred_canvas_zoom(ctx: &Context) -> Option<DeferredCanvasZoom> {
    let deferred_key = deferred_canvas_id(ctx);
    ctx.data_mut(|data| {
        let zoom = data.get_temp(deferred_key);
        data.remove::<DeferredCanvasZoom>(deferred_key);
        zoom
    })
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
    let generation = gesture_generation(ctx);
    ctx.data_mut(|data| {
        data.insert_temp(
            id,
            GestureLatch {
                owner,
                last_seen: now,
                kind,
                generation,
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
mod tests;
