use egui::{Context, Event, Id, InputState, TouchPhase, Vec2};
use horizon_core::PanelId;

use crate::input::TerminalInputEvent;

// X11 scroll events have no start/end phases. An idle gap separates their gestures.
const SCROLL_GESTURE_IDLE_SECONDS: f64 = 0.15;

#[derive(Clone, Default)]
struct ScrollGesture {
    canvas_owned: Option<bool>,
    /// The panel the gesture latched onto, so chaining is decided for its
    /// owner rather than whatever the pointer happens to be over later.
    owner: Option<PanelId>,
    last_motion_at: f64,
    has_touch_phase: bool,
}

#[derive(Default)]
pub(super) struct ScrollRouting {
    pub(super) pans_canvas: bool,
    owns_smooth_scroll: bool,
    claimed_wheels: Vec<usize>,
}

impl ScrollGesture {
    /// Bind the gesture to the surface it started over. A gesture with no
    /// panel under the pointer belongs to the canvas outright.
    fn latch(&mut self, panel: Option<PanelId>) {
        self.canvas_owned = Some(panel.is_none());
        self.owner = panel;
    }

    fn route(
        &mut self,
        input: &InputState,
        panel: Option<PanelId>,
        allowed: bool,
        exhausted: &impl Fn(PanelId) -> bool,
    ) -> ScrollRouting {
        if !allowed || !input.focused || input.pointer.any_pressed() {
            *self = Self::default();
            return ScrollRouting::default();
        }

        let mut routing = ScrollRouting::default();
        let mut has_canvas_motion = false;
        for (index, (delta, phase)) in input
            .events
            .iter()
            .filter_map(|event| match event {
                Event::MouseWheel { delta, phase, .. } => Some((*delta, *phase)),
                _ => None,
            })
            .enumerate()
        {
            match phase {
                TouchPhase::Start => {
                    self.latch(panel);
                    self.last_motion_at = input.time;
                    self.has_touch_phase = true;
                }
                TouchPhase::Move if delta != Vec2::ZERO => {
                    if self.canvas_owned.is_none()
                        || (!self.has_touch_phase && input.time - self.last_motion_at > SCROLL_GESTURE_IDLE_SECONDS)
                    {
                        self.latch(panel);
                    }
                    self.last_motion_at = input.time;
                }
                TouchPhase::End | TouchPhase::Cancel | TouchPhase::Move => {}
            }
            // Scroll chaining: a gesture latched to a panel moves to the canvas
            // once *that* panel is at its scroll extent, and stays there for
            // the rest of the gesture. The owner decides, so a pointer that
            // drifts over some other panel mid-gesture changes nothing.
            if self.canvas_owned == Some(false) && self.owner.is_some_and(exhausted) {
                self.canvas_owned = Some(true);
            }
            if self.canvas_owned == Some(true) {
                routing.claimed_wheels.push(index);
                has_canvas_motion |= delta != Vec2::ZERO;
            }
            if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
                *self = Self::default();
            }
        }

        routing.owns_smooth_scroll = self.canvas_owned == Some(true);
        routing.pans_canvas =
            routing.owns_smooth_scroll && (has_canvas_motion || input.smooth_scroll_delta != Vec2::ZERO);
        routing
    }
}

/// Route this frame's scroll. `panel` is the panel under the pointer, used only
/// when a gesture latches; `exhausted` reports whether a panel can still absorb
/// the scroll and is always asked about the gesture's own owner.
pub(super) fn route_canvas_scroll(
    ctx: &Context,
    panel: Option<PanelId>,
    allowed: bool,
    exhausted: impl Fn(PanelId) -> bool,
) -> ScrollRouting {
    let id = Id::new(("canvas_scroll_gesture", ctx.viewport_id()));
    let mut gesture = ctx.data_mut(|data| data.get_temp::<ScrollGesture>(id).unwrap_or_default());
    let routing = ctx.input(|input| gesture.route(input, panel, allowed, &exhausted));
    ctx.data_mut(|data| data.insert_temp(id, gesture));
    routing
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
