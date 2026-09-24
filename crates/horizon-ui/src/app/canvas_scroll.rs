use egui::{Context, Event, Id, InputOptions, InputState, Modifiers, MouseWheelUnit, TouchPhase, Vec2};
use horizon_core::PanelId;

use crate::input::TerminalInputEvent;

// X11 scroll events have no start/end phases. An idle gap separates their gestures.
const SCROLL_GESTURE_IDLE_SECONDS: f64 = 0.15;

/// The surface under the pointer, which a scroll gesture starting now latches
/// onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScrollTarget {
    Canvas,
    /// A board panel, which hands its gesture on to the canvas once it
    /// reaches its scroll extent.
    Panel(PanelId),
    /// A canvas-drawn surface with no known scroll extent, such as a cloud
    /// runtime card. It keeps every gesture it starts.
    Surface,
}

/// One wheel event, carrying what the terminal needs to apply it.
#[derive(Clone, Copy, Debug)]
pub(super) struct WheelStep {
    pub(super) delta: Vec2,
    pub(super) unit: MouseWheelUnit,
    pub(super) modifiers: Modifiers,
}

#[derive(Clone, Default)]
struct ScrollGesture {
    canvas_owned: Option<bool>,
    /// The panel the gesture latched onto, so chaining is decided for its
    /// owner rather than whatever the pointer happens to be over later.
    owner: Option<PanelId>,
    last_motion_at: f64,
    has_touch_phase: bool,
    /// Claimed wheel motion still being eased in, so a claimed mouse-wheel
    /// notch pans as smoothly as egui would have scrolled it.
    pan_backlog: Vec2,
    /// Already computed motion held until panel wheels can render without
    /// moving their target geometry or disabling panel interaction.
    deferred_pan: Vec2,
}

#[derive(Default)]
pub(super) struct ScrollRouting {
    pub(super) pans_canvas: bool,
    /// Canvas motion from the claimed events only. egui's frame-wide scroll
    /// also carries events a panel absorbed before the gesture chained.
    pub(super) pan: Vec2,
    owns_smooth_scroll: bool,
    claimed_wheels: Vec<usize>,
    absorbed_wheels: Vec<(usize, PanelId, WheelStep)>,
}

impl ScrollGesture {
    /// Bind the gesture to the surface it started over. A gesture that starts
    /// over empty canvas belongs to the canvas outright. Returns the previous
    /// gesture's claimed motion still easing in, which lands rather than being
    /// dropped.
    fn latch(&mut self, target: ScrollTarget) -> Vec2 {
        self.canvas_owned = Some(target == ScrollTarget::Canvas);
        self.owner = match target {
            ScrollTarget::Panel(panel) => Some(panel),
            ScrollTarget::Canvas | ScrollTarget::Surface => None,
        };
        std::mem::take(&mut self.pan_backlog)
    }

    /// Queue one claimed event's motion exactly as egui scrolls it: in points,
    /// turned by the scroll-axis modifiers, and eased in over a few frames
    /// unless it is a precise trackpad step.
    fn claim_motion(&mut self, input: &InputState, options: &InputOptions, step: WheelStep, pan: &mut Vec2) {
        let mut delta = match step.unit {
            MouseWheelUnit::Point => step.delta,
            MouseWheelUnit::Line => options.line_scroll_speed * step.delta,
            MouseWheelUnit::Page => input.viewport_rect().height() * step.delta,
        };
        let horizontal = step.modifiers.matches_any(options.horizontal_scroll_modifier);
        let vertical = step.modifiers.matches_any(options.vertical_scroll_modifier);
        if horizontal && !vertical {
            delta = Vec2::new(delta.x + delta.y, 0.0);
        } else if vertical && !horizontal {
            delta = Vec2::new(0.0, delta.x + delta.y);
        }
        if self.has_touch_phase || (step.unit == MouseWheelUnit::Point && delta.length() < 8.0) {
            *pan += delta;
        } else {
            self.pan_backlog += delta;
        }
    }

    /// egui's own easing: 90% of a queued step lands within 0.1 s.
    fn ease_backlog(&mut self, dt: f32) -> Vec2 {
        let t = egui::emath::exponential_smooth_factor(0.90, 0.1, dt.min(0.1));
        let mut applied = Vec2::ZERO;
        for axis in 0..2 {
            applied[axis] = if self.pan_backlog[axis].abs() < 1.0 {
                self.pan_backlog[axis]
            } else {
                t * self.pan_backlog[axis]
            };
            self.pan_backlog[axis] -= applied[axis];
        }
        applied
    }

    fn route(
        &mut self,
        input: &InputState,
        options: &InputOptions,
        target: ScrollTarget,
        allowed: bool,
        exhausted: &mut impl FnMut(PanelId, WheelStep) -> bool,
    ) -> ScrollRouting {
        if !allowed || !input.focused || input.pointer.any_pressed() {
            *self = Self::default();
            return ScrollRouting::default();
        }

        let mut routing = ScrollRouting {
            pan: std::mem::take(&mut self.deferred_pan),
            ..ScrollRouting::default()
        };
        let mut has_canvas_motion = false;
        for (index, (step, phase)) in input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    modifiers,
                } => Some((
                    WheelStep {
                        delta: *delta,
                        unit: *unit,
                        modifiers: *modifiers,
                    },
                    *phase,
                )),
                _ => None,
            })
            .enumerate()
        {
            // A Ctrl/Cmd wheel is a zoom step, exactly as the terminal treats
            // it: whatever its phase or delta, it neither starts, continues,
            // chains nor moves a pan, and stays in the stream. Judged per
            // event, since one frame can mix both. Its phase still marks the
            // contact: `End` or `Cancel` ends the pan gesture, and `Start`
            // opens a new one that stays unowned until a plain event moves.
            if step.modifiers.ctrl || step.modifiers.command {
                if phase != TouchPhase::Move {
                    // The old gesture's claimed motion still lands, in full.
                    routing.pan += self.pan_backlog;
                    *self = Self::default();
                    if phase == TouchPhase::Start {
                        self.last_motion_at = input.time;
                        self.has_touch_phase = true;
                    }
                }
                continue;
            }
            let delta = step.delta;
            match phase {
                TouchPhase::Start => {
                    routing.pan += self.latch(target);
                    self.last_motion_at = input.time;
                    self.has_touch_phase = true;
                }
                TouchPhase::Move if delta != Vec2::ZERO => {
                    if self.canvas_owned.is_none()
                        || (!self.has_touch_phase && input.time - self.last_motion_at > SCROLL_GESTURE_IDLE_SECONDS)
                    {
                        routing.pan += self.latch(target);
                    }
                    self.last_motion_at = input.time;
                }
                TouchPhase::End | TouchPhase::Cancel | TouchPhase::Move => {}
            }
            // Scroll chaining: a gesture latched to a panel moves to the canvas
            // once *that* panel is at its scroll extent, and stays there for
            // the rest of the gesture. The owner decides, so a pointer that
            // drifts over some other panel mid-gesture changes nothing. Only
            // an event carrying motion can chain: a phased gesture can open
            // with a zero-delta `Start`, which has no direction to be
            // exhausted in.
            // Each event is judged by its own delta, as the terminal applies
            // it, never by the frame's sum, where opposing events cancel out,
            // and in order, so `exhausted` can account for the events before.
            if delta != Vec2::ZERO
                && self.canvas_owned == Some(false)
                && let Some(owner) = self.owner
            {
                if exhausted(owner, step) {
                    self.canvas_owned = Some(true);
                } else {
                    routing.absorbed_wheels.push((index, owner, step));
                }
            }
            if self.canvas_owned == Some(true) {
                routing.claimed_wheels.push(index);
                // Any phase can carry motion: winit's Wayland backend opens a
                // touchpad gesture with a `Start` that already has a delta.
                if delta != Vec2::ZERO {
                    has_canvas_motion = true;
                    self.claim_motion(input, options, step, &mut routing.pan);
                }
            }
            if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
                // The lifted gesture's claimed motion still lands, in full;
                // only its ownership ends with it.
                routing.pan += self.pan_backlog;
                *self = Self::default();
            }
        }

        let canvas_owned = self.canvas_owned == Some(true);
        // egui scroll areas use smoothed motion rather than the raw events
        // removed below. Keep a displaced owner's easing away from the new
        // hover target, including idle frames with no wheel events to remove.
        routing.owns_smooth_scroll =
            canvas_owned || self.owner.is_some_and(|owner| target != ScrollTarget::Panel(owner));
        if canvas_owned {
            routing.pan += self.ease_backlog(input.stable_dt);
        }
        routing.pans_canvas = has_canvas_motion || routing.pan != Vec2::ZERO;
        routing
    }
}

/// Route this frame's scroll. `target` is the surface under the pointer, used
/// only when a gesture latches. `exhausted` reports whether a panel has run
/// out of scroll for one wheel event; it is always asked about the gesture's
/// own owner, once per moving event in order, until the gesture chains.
pub(super) fn route_canvas_scroll(
    ctx: &Context,
    target: ScrollTarget,
    allowed: bool,
    mut exhausted: impl FnMut(PanelId, WheelStep) -> bool,
) -> ScrollRouting {
    let id = Id::new(("canvas_scroll_gesture", ctx.viewport_id()));
    let mut gesture = ctx.data_mut(|data| data.get_temp::<ScrollGesture>(id).unwrap_or_default());
    let options = ctx.options(|options| options.input_options);
    let routing = ctx.input(|input| gesture.route(input, &options, target, allowed, &mut exhausted));
    if gesture.pan_backlog != Vec2::ZERO {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, gesture));
    routing
}

/// Suppressed viewports discard canvas ownership and easing. Phased zoom stays
/// rejected until an observed boundary, including boundaries handled here while
/// a dialog or fullscreen view bypasses normal canvas input.
pub(super) fn reset_canvas_scroll(ctx: &Context) {
    let viewport = ctx.viewport_id();
    let boundary = ctx.input(|input| {
        input.raw.events.iter().rev().find_map(|event| match event {
            Event::MouseWheel { phase, .. } if *phase != TouchPhase::Move => Some(*phase),
            _ => None,
        })
    });
    ctx.data_mut(|data| {
        data.remove::<ScrollGesture>(Id::new(("canvas_scroll_gesture", viewport)));
        let zoom = data.get_temp_mut_or_default::<WheelZoom>(Id::new(("canvas_wheel_zoom", viewport)));
        zoom.reject_contact();
        match boundary {
            Some(TouchPhase::Start) => zoom.contact = ZoomContact::Rejected,
            Some(TouchPhase::End | TouchPhase::Cancel) => zoom.contact = ZoomContact::Unphased,
            Some(TouchPhase::Move) | None => {}
        }
    });
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ZoomContact {
    #[default]
    Unphased,
    Accepted,
    Rejected,
}

/// Ctrl/Cmd wheel zoom still being eased in, and ownership of a phased contact.
#[derive(Clone, Copy, Default)]
struct WheelZoom {
    backlog: f32,
    contact: ZoomContact,
}

impl WheelZoom {
    fn reject_contact(&mut self) {
        self.backlog = 0.0;
        if self.contact != ZoomContact::Unphased {
            self.contact = ZoomContact::Rejected;
        }
    }
}

/// Canvas zoom from this frame's input. egui sums a frame's wheel deltas and
/// classifies the total as all zoom or all scroll, so a frame mixing a plain
/// and a Ctrl/Cmd wheel would misroute one of them. Zoom-modified wheels are
/// taken here one event at a time instead, converted and eased exactly as
/// egui zooms with them, while native pinch and multi-touch pass through as
/// egui reports them. Unlike egui, a `Start` or `End` that carries a delta
/// zooms too. Plain wheels are left to [`route_canvas_scroll`]. Off the
/// canvas nothing queues and any easing step is dropped, so a notch over the
/// sidebar never zooms the canvas once the pointer moves onto it.
pub(super) fn canvas_zoom_delta(ctx: &Context, over_canvas: bool) -> f32 {
    let id = Id::new(("canvas_wheel_zoom", ctx.viewport_id()));
    let mut state = ctx.data_mut(|data| data.get_temp::<WheelZoom>(id).unwrap_or_default());
    let options = ctx.options(|options| options.input_options);
    let zoom = ctx.input(|input| {
        let allowed = over_canvas && input.focused;
        if !allowed {
            state.reject_contact();
        }
        let mut factor = 1.0;
        let mut immediate = 0.0;
        for event in &input.raw.events {
            match event {
                Event::Zoom(delta) if delta.is_finite() => factor *= *delta,
                Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    modifiers,
                } => {
                    if *phase == TouchPhase::Start {
                        immediate += std::mem::take(&mut state.backlog);
                        state.contact = if allowed {
                            ZoomContact::Accepted
                        } else {
                            ZoomContact::Rejected
                        };
                    }
                    // Any phase can carry motion, as for the pan: a Wayland
                    // touchpad gesture opens with a `Start` that has a delta.
                    if allowed
                        && state.contact != ZoomContact::Rejected
                        && *delta != Vec2::ZERO
                        && modifiers.matches_any(options.zoom_modifier)
                    {
                        let points = match unit {
                            MouseWheelUnit::Point => *delta,
                            MouseWheelUnit::Line => options.line_scroll_speed * *delta,
                            MouseWheelUnit::Page => input.viewport_rect().height() * *delta,
                        };
                        if state.contact == ZoomContact::Accepted
                            || (*unit == MouseWheelUnit::Point && points.length() < 8.0)
                        {
                            immediate += points.x + points.y;
                        } else {
                            state.backlog += points.x + points.y;
                        }
                    }
                    if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
                        immediate += std::mem::take(&mut state.backlog);
                        state.contact = ZoomContact::Unphased;
                    }
                }
                _ => {}
            }
        }
        if !allowed {
            return 1.0;
        }
        let t = egui::emath::exponential_smooth_factor(0.90, 0.1, input.stable_dt.min(0.1));
        let eased = if state.backlog.abs() < 1.0 {
            state.backlog
        } else {
            t * state.backlog
        };
        state.backlog -= eased;
        let wheel = (options.scroll_zoom_speed * (immediate + eased)).exp();
        input.multi_touch().map_or(factor * wheel, |touch| touch.zoom_delta)
    });
    if state.backlog != 0.0 {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, state));
    zoom
}

impl ScrollRouting {
    /// A latched panel's wheel must never leak to a different hover target.
    pub(super) fn discard_displaced_wheels(&mut self, target: ScrollTarget) {
        for &(index, owner, _) in &self.absorbed_wheels {
            if target != ScrollTarget::Panel(owner) {
                self.claimed_wheels.push(index);
            }
        }
        self.claimed_wheels.sort_unstable();
    }

    /// Keep the original geometry and interaction enabled while a panel takes
    /// its wheels through the normal renderer. Continuous panel input can hold
    /// the old canvas tail until the first idle frame; no displacement is lost.
    pub(super) fn defer_for_panel_delivery(&mut self, ctx: &Context) {
        if !self.pans_canvas
            || !self
                .absorbed_wheels
                .iter()
                .any(|(index, _, _)| self.claimed_wheels.binary_search(index).is_err())
        {
            return;
        }
        let id = Id::new(("canvas_scroll_gesture", ctx.viewport_id()));
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<ScrollGesture>(id).deferred_pan += std::mem::take(&mut self.pan);
        });
        self.pans_canvas = false;
        self.owns_smooth_scroll = false;
        ctx.request_repaint();
    }

    pub(super) fn consume(&self, ctx: &Context, terminal_events: &mut Vec<TerminalInputEvent>) {
        ctx.input_mut(|input| {
            discard_claimed_wheels(&mut input.events, &self.claimed_wheels, |event| event);
            if self.owns_smooth_scroll {
                input.smooth_scroll_delta = Vec2::ZERO;
            }
        });
        discard_claimed_wheels(terminal_events, &self.claimed_wheels, |input| &input.event);
    }
}

fn discard_claimed_wheels<T>(events: &mut Vec<T>, claimed: &[usize], event: impl Fn(&T) -> &Event) {
    if claimed.is_empty() {
        return;
    }
    let mut claims = claimed.iter().copied().peekable();
    let mut index = 0;
    events.retain(|input| {
        if !matches!(event(input), Event::MouseWheel { .. }) {
            return true;
        }
        let consumed = claims.peek() == Some(&index);
        if consumed {
            claims.next();
        }
        index += 1;
        !consumed
    });
}

#[cfg(test)]
mod tests;
