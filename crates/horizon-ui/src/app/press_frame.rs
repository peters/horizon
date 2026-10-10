//! Give a pointer press its own frame when pointer input follows it in one batch.
//!
//! egui hit-tests once per frame, at the last pointer position of the frame. When the
//! window system delivers a press together with the motion after it (a slow frame, or a
//! quick drag that starts without a pause), egui looks for the pressed widget where the
//! pointer ends up instead of where it went down. A short target, such as a panel
//! titlebar dragged vertically, then never gets the drag.

use egui::Event;

#[derive(Default)]
pub(super) struct PressFrame {
    /// Pointer input that arrived after a press, for the next frame.
    deferred: Vec<Event>,
}

impl PressFrame {
    /// Prepends the input held back from the last frame, then holds back the pointer input
    /// that follows the first press when that input moves the pointer.
    ///
    /// Other input stays in this frame, in its order, so keyboard input stays paired with
    /// what the platform observed for this frame. Returns whether input was held back; the
    /// caller must then request a frame at once.
    pub(super) fn split(&mut self, events: &mut Vec<Event>) -> bool {
        if !self.deferred.is_empty() {
            let mut carried = std::mem::take(&mut self.deferred);
            carried.append(events);
            *events = carried;
        }
        let Some(press) = events
            .iter()
            .position(|event| matches!(event, Event::PointerButton { pressed: true, .. }))
        else {
            return false;
        };
        let rest = &events[press + 1..];
        if !rest.iter().any(|event| matches!(event, Event::PointerMoved(_))) {
            return false;
        }
        let (pointer, other): (Vec<_>, Vec<_>) = events.drain(press + 1..).partition(is_pointer_input);
        events.extend(other);
        self.deferred = pointer;
        true
    }
}

fn is_pointer_input(event: &Event) -> bool {
    matches!(
        event,
        Event::PointerMoved(_)
            | Event::MouseMoved(_)
            | Event::PointerButton { .. }
            | Event::PointerGone
            | Event::MouseWheel { .. }
            | Event::Touch { .. }
    )
}

#[cfg(test)]
mod tests;
