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
}

#[derive(Default)]
pub(super) struct ScrollRouting {
    pub(super) pans_canvas: bool,
    /// Canvas motion from the claimed events only. egui's frame-wide scroll
    /// also carries events a panel absorbed before the gesture chained.
    pub(super) pan: Vec2,
    owns_smooth_scroll: bool,
    claimed_wheels: Vec<usize>,
}

impl ScrollGesture {
    /// Bind the gesture to the surface it started over. A gesture that starts
    /// over empty canvas belongs to the canvas outright.
    fn latch(&mut self, target: ScrollTarget) {
        self.canvas_owned = Some(target == ScrollTarget::Canvas);
        self.owner = match target {
            ScrollTarget::Panel(panel) => Some(panel),
            ScrollTarget::Canvas | ScrollTarget::Surface => None,
        };
        self.pan_backlog = Vec2::ZERO;
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

        let mut routing = ScrollRouting::default();
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
            // it: it neither starts, continues nor chains a pan, and stays in
            // the stream. Judged per event, since one frame can mix both.
            if phase == TouchPhase::Move && (step.modifiers.ctrl || step.modifiers.command) {
                continue;
            }
            let delta = step.delta;
            match phase {
                TouchPhase::Start => {
                    self.latch(target);
                    self.last_motion_at = input.time;
                    self.has_touch_phase = true;
                }
                TouchPhase::Move if delta != Vec2::ZERO => {
                    if self.canvas_owned.is_none()
                        || (!self.has_touch_phase && input.time - self.last_motion_at > SCROLL_GESTURE_IDLE_SECONDS)
                    {
                        self.latch(target);
                    }
                    self.last_motion_at = input.time;
                }
                TouchPhase::End | TouchPhase::Cancel | TouchPhase::Move => {}
            }
            // Scroll chaining: a gesture latched to a panel moves to the canvas
            // once *that* panel is at its scroll extent, and stays there for
            // the rest of the gesture. The owner decides, so a pointer that
            // drifts over some other panel mid-gesture changes nothing. Only
            // an event carrying motion can chain: a phased gesture opens with a
            // zero-delta `Start`, which has no direction to be exhausted in.
            // Each event is judged by its own delta, as the terminal applies
            // it, never by the frame's sum, where opposing events cancel out,
            // and in order, so `exhausted` can account for the events before.
            if delta != Vec2::ZERO
                && self.canvas_owned == Some(false)
                && self.owner.is_some_and(|owner| exhausted(owner, step))
            {
                self.canvas_owned = Some(true);
            }
            if self.canvas_owned == Some(true) {
                routing.claimed_wheels.push(index);
                has_canvas_motion |= delta != Vec2::ZERO;
                if phase == TouchPhase::Move {
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

        routing.owns_smooth_scroll = self.canvas_owned == Some(true);
        if routing.owns_smooth_scroll {
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

/// Ctrl/Cmd wheel zoom still being eased in, and whether a phased wheel
/// gesture is in progress.
#[derive(Clone, Copy, Default)]
struct WheelZoom {
    backlog: f32,
    in_touch: bool,
}

/// Canvas zoom from this frame's input. egui sums a frame's wheel deltas and
/// classifies the total as all zoom or all scroll, so a frame mixing a plain
/// and a Ctrl/Cmd wheel would misroute one of them. Zoom-modified wheels are
/// taken here one event at a time instead, converted and eased exactly as
/// egui zooms with them, while native pinch and multi-touch pass through as
/// egui reports them. Plain wheels are left to [`route_canvas_scroll`].
pub(super) fn canvas_zoom_delta(ctx: &Context) -> f32 {
    let id = Id::new(("canvas_wheel_zoom", ctx.viewport_id()));
    let mut state = ctx.data_mut(|data| data.get_temp::<WheelZoom>(id).unwrap_or_default());
    let options = ctx.options(|options| options.input_options);
    let zoom = ctx.input(|input| {
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
                } => match phase {
                    TouchPhase::Start => state.in_touch = true,
                    TouchPhase::End | TouchPhase::Cancel => {
                        immediate += std::mem::take(&mut state.backlog);
                        state.in_touch = false;
                    }
                    TouchPhase::Move if modifiers.matches_any(options.zoom_modifier) => {
                        let points = match unit {
                            MouseWheelUnit::Point => *delta,
                            MouseWheelUnit::Line => options.line_scroll_speed * *delta,
                            MouseWheelUnit::Page => input.viewport_rect().height() * *delta,
                        };
                        if state.in_touch || (*unit == MouseWheelUnit::Point && points.length() < 8.0) {
                            immediate += points.x + points.y;
                        } else {
                            state.backlog += points.x + points.y;
                        }
                    }
                    TouchPhase::Move => {}
                },
                _ => {}
            }
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
