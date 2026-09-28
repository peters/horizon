//! Reserve modified canvas clicks until their single/double-click intent is known.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use egui::{Event, InputOptions, PointerButton, Pos2, RawInput, ViewportId};

use super::HorizonApp;

pub(super) struct CanvasGesture {
    clock: Instant,
    session_id: Option<String>,
    pending: Option<Pending>,
    completed: Option<Pos2>,
    forwarded_down: bool,
    replayed_origin: Option<Pos2>,
    queued: VecDeque<Frame>,
    cancelled_down: bool,
    other_buttons: [bool; 5],
}

struct Frame {
    time: f64,
    events: Vec<Event>,
}

struct Pending {
    origin: Pos2,
    modifiers: egui::Modifiers,
    pressed_at: f64,
    released_at: Option<f64>,
    events: Vec<Event>,
}

impl Default for CanvasGesture {
    fn default() -> Self {
        Self {
            clock: Instant::now(),
            session_id: None,
            pending: None,
            completed: None,
            forwarded_down: false,
            replayed_origin: None,
            queued: VecDeque::new(),
            cancelled_down: false,
            other_buttons: [false; 5],
        }
    }
}

impl CanvasGesture {
    pub(super) fn discard(&mut self) {
        *self = Self::default();
    }

    pub(super) fn take_completed(&mut self) -> Option<Pos2> {
        self.completed.take()
    }

    fn flush(&mut self, output: &mut Vec<Event>) {
        if let Some(pending) = self.pending.take() {
            if pending.released_at.is_some() {
                self.replayed_origin = Some(pending.origin);
            }
            self.forwarded_down = pending.released_at.is_none();
            output.extend(pending.events);
        }
    }

    fn cancel(&mut self) {
        self.forwarded_down = false;
        if let Some(pending) = self.pending.take() {
            self.cancelled_down = pending.released_at.is_none();
        }
        self.completed = None;
    }

    fn filter(
        &mut self,
        raw: &mut RawInput,
        options: &InputOptions,
        eligible: impl Fn(Pos2) -> bool,
    ) -> Option<Duration> {
        let now = raw.time.unwrap_or_else(|| self.clock.elapsed().as_secs_f64());
        let lost_focus = !raw.focused
            || raw
                .events
                .iter()
                .any(|event| matches!(event, Event::WindowFocused(false)));
        if lost_focus || self.replayed_origin.is_some_and(|origin| !eligible(origin)) {
            self.cancel();
            for frame in &mut self.queued {
                frame.events.retain(|event| {
                    !matches!(
                        event,
                        Event::PointerButton { .. } | Event::PointerMoved(_) | Event::MouseMoved(_)
                    )
                });
            }
        }
        if lost_focus {
            self.other_buttons.fill(false);
        }
        if self.pending.is_none()
            && !self.forwarded_down
            && !self.cancelled_down
            && self.queued.is_empty()
            && self.replayed_origin.is_none()
            && !raw.events.iter().any(|event| {
                matches!(event,
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers }
                    if (modifiers.ctrl || modifiers.command) && eligible(*pos))
            })
        {
            for event in &raw.events {
                self.track_other_button(event);
            }
            return None;
        }
        if !raw.events.is_empty() || self.queued.is_empty() {
            self.queued.push_back(Frame {
                time: now,
                events: std::mem::take(&mut raw.events),
            });
        }
        let frame = self.queued.pop_front()?;
        let mut output = Vec::with_capacity(frame.events.len());
        if let Some(pending) = &self.pending {
            if lost_focus || !eligible(pending.origin) {
                self.cancel();
            } else if frame.time >= Self::deadline(pending, options) {
                self.flush(&mut output);
            }
        }
        let mut events = frame.events.into_iter();
        while let Some(event) = events.next() {
            if self.replayed_origin.is_some() || self.interrupts_completed(&event, frame.time, options, &eligible) {
                if self.replayed_origin.is_none() {
                    self.flush(&mut output);
                }
                self.queued.push_front(Frame {
                    time: frame.time,
                    events: std::iter::once(event).chain(events).collect(),
                });
                break;
            }
            self.event(event, frame.time, options, &eligible, &mut output);
        }
        raw.events = output;
        if self.replayed_origin.is_some() || !self.queued.is_empty() {
            return Some(Duration::ZERO);
        }
        self.pending
            .as_ref()
            .map(|pending| Duration::from_secs_f64((Self::deadline(pending, options) - now).max(0.0)))
    }

    fn deadline(pending: &Pending, options: &InputOptions) -> f64 {
        pending
            .released_at
            .map_or(pending.pressed_at + options.max_click_duration, |at| {
                at + options.max_double_click_delay
            })
    }

    fn interrupts_completed(
        &self,
        event: &Event,
        now: f64,
        options: &InputOptions,
        eligible: &impl Fn(Pos2) -> bool,
    ) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let Some(released) = pending.released_at else {
            return false;
        };
        match event {
            Event::ModifiersChanged(modifiers) if *modifiers == pending.modifiers => false,
            Event::MouseMoved(_) => false,
            Event::PointerMoved(pos) => pending.origin.distance(*pos) >= options.max_click_dist || !eligible(*pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            } => {
                !(modifiers.ctrl || modifiers.command)
                    || !eligible(*pos)
                    || now - released >= options.max_double_click_delay
                    || pending.origin.distance(*pos) >= options.max_click_dist
            }
            _ => true,
        }
    }

    fn track_other_button(&mut self, event: &Event) {
        if matches!(event, Event::WindowFocused(false)) {
            self.other_buttons.fill(false);
        }
        if let Event::PointerButton { button, pressed, .. } = event
            && *button != PointerButton::Primary
        {
            self.other_buttons[*button as usize] = *pressed;
        }
    }

    fn event(
        &mut self,
        event: Event,
        now: f64,
        options: &InputOptions,
        eligible: &impl Fn(Pos2) -> bool,
        output: &mut Vec<Event>,
    ) {
        self.track_other_button(&event);
        match &event {
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            } => {
                if self.cancelled_down && !pressed {
                    self.cancelled_down = false;
                    return;
                }
                if *pressed {
                    self.cancelled_down = false;
                }
                if self.forwarded_down {
                    self.forwarded_down = *pressed;
                    output.push(event);
                    return;
                }
                if *pressed
                    && (modifiers.ctrl || modifiers.command)
                    && eligible(*pos)
                    && !self.other_buttons.iter().any(|down| *down)
                {
                    if let Some(pending) = &mut self.pending
                        && pending
                            .released_at
                            .is_some_and(|at| now - at < options.max_double_click_delay)
                        && pending.origin.distance(*pos) < options.max_click_dist
                    {
                        // Recognize on the second press, so neither click is ever replayed
                        // as a same-frame pair if the second button is held down.
                        self.completed = Some(*pos);
                        self.pending = None;
                        self.cancelled_down = true;
                        return;
                    }
                    self.flush(output);
                    self.pending = Some(Pending {
                        origin: *pos,
                        modifiers: *modifiers,
                        pressed_at: now,
                        released_at: None,
                        events: vec![event],
                    });
                    return;
                }
                if !pressed && let Some(pending) = &mut self.pending {
                    if now - pending.pressed_at >= options.max_click_duration {
                        self.flush(output);
                        self.forwarded_down = false;
                        output.push(event);
                    } else {
                        pending.released_at = Some(now);
                        pending.events.push(event);
                    }
                    return;
                }
                self.flush(output);
                self.forwarded_down = *pressed;
                output.push(event);
            }
            Event::PointerMoved(pos) => {
                if let Some(pending) = &mut self.pending {
                    if pending.origin.distance(*pos) >= options.max_click_dist || !eligible(*pos) {
                        self.flush(output);
                    } else {
                        if matches!(pending.events.last(), Some(Event::PointerMoved(_))) {
                            pending.events.pop();
                        }
                        pending.events.push(event);
                        return;
                    }
                }
                output.push(event);
            }
            Event::ModifiersChanged(modifiers)
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.modifiers == *modifiers) =>
            {
                output.push(event);
            }
            Event::MouseMoved(_) => output.push(event),
            _ => {
                self.flush(output);
                output.push(event);
            }
        }
    }
}

impl HorizonApp {
    pub(super) fn canvas_gesture_enabled(&self) -> bool {
        self.fullscreen_panel.is_none()
            && !self.host_dialog_open()
            && self.settings.is_none()
            && self.session_manager.is_none()
            && self.startup_chooser.is_none()
            && self.command_palette.is_none()
            && !self
                .search_overlay
                .as_ref()
                .is_some_and(crate::search_overlay::SearchOverlay::input_focused)
            && self.dir_picker.is_none()
            && !self.root_viewport_stabilization_blocks_interaction()
    }

    pub(super) fn filter_canvas_gesture(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        if raw.viewport_id != ViewportId::ROOT {
            return;
        }
        self.canvas_gesture.completed = None;
        let session_id = self.active_session.as_ref().map(|session| session.session_id.as_str());
        if self.canvas_gesture.session_id.as_deref() != session_id {
            self.canvas_gesture.cancel();
            self.canvas_gesture.queued.clear();
            self.canvas_gesture.session_id = session_id.map(str::to_owned);
        }
        let enabled = raw.focused && self.canvas_gesture_enabled();
        let canvas = self.canvas_rect(ctx);
        let exclusions = self.overlay_exclusion_zones(ctx);
        let options = ctx.options(|options| options.input_options);
        // A completed deferred click gets a frame of its own. Clear only its
        // synthetic click history while idle, before delivering later input.
        let mut restore_pos = None;
        let ending_replay = self.canvas_gesture.replayed_origin.is_some();
        if let Some(origin) = self.canvas_gesture.replayed_origin.take() {
            if !enabled || !canvas.contains(origin) || exclusions.contains(origin) {
                self.canvas_gesture.cancel();
                self.canvas_gesture.queued.clear();
            }
            ctx.input_mut(|input| {
                let pos = input.pointer.latest_pos();
                if !input.pointer.any_down() {
                    input.pointer = egui::PointerState::default();
                    restore_pos = pos;
                }
            });
        }
        if let Some(delay) = self.canvas_gesture.filter(raw, &options, |pos| {
            enabled && canvas.contains(pos) && !exclusions.contains(pos)
        }) {
            ctx.request_repaint_after(delay);
        }
        if ending_replay || self.canvas_gesture.replayed_origin.is_some() {
            for state in self.panel_render_caches.browser_ui_state.values_mut() {
                state.clear_click_history();
            }
        }
        if let Some(pos) = restore_pos {
            raw.events.insert(0, Event::PointerMoved(pos));
        }
    }
}

#[cfg(test)]
mod tests;
