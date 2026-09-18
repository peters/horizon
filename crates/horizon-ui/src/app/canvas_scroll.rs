use egui::{Context, Event, Id, InputState, TouchPhase, Vec2};

use crate::input::TerminalInputEvent;

// X11 scroll events have no start/end phases. An idle gap separates their gestures.
const SCROLL_GESTURE_IDLE_SECONDS: f64 = 0.15;

#[derive(Clone, Default)]
struct ScrollGesture {
    canvas_owned: Option<bool>,
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
    fn route(&mut self, input: &InputState, starts_on_canvas: bool, allowed: bool) -> ScrollRouting {
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
                    self.canvas_owned = Some(starts_on_canvas);
                    self.last_motion_at = input.time;
                    self.has_touch_phase = true;
                }
                TouchPhase::Move if delta != Vec2::ZERO => {
                    if self.canvas_owned.is_none()
                        || (!self.has_touch_phase && input.time - self.last_motion_at > SCROLL_GESTURE_IDLE_SECONDS)
                    {
                        self.canvas_owned = Some(starts_on_canvas);
                    }
                    self.last_motion_at = input.time;
                }
                TouchPhase::End | TouchPhase::Cancel | TouchPhase::Move => {}
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

pub(super) fn route_canvas_scroll(ctx: &Context, starts_on_canvas: bool, allowed: bool) -> ScrollRouting {
    let id = Id::new(("canvas_scroll_gesture", ctx.viewport_id()));
    let mut gesture = ctx.data_mut(|data| data.get_temp::<ScrollGesture>(id).unwrap_or_default());
    let routing = ctx.input(|input| gesture.route(input, starts_on_canvas, allowed));
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
