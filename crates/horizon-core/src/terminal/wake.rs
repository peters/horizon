//! The wake that shows the echo of a person's key press in the next frame.
//!
//! A terminal panel paints on the poll of the window, which backs off to a
//! second while the board is quiet. That poll is right for a passive panel, but
//! a person who types waits for the echo of every key, so the panel they type
//! into asks for one frame when the bytes of their echo arrive.
//!
//! The request is armed by the input of a person and spent by the first output,
//! so a program that prints without end never drives the window: at most one
//! frame follows each key press, and only for a panel that a frame is drawing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// The callback that asks the window for the frame that shows new output.
pub type OutputWake = Arc<dyn Fn() + Send + Sync>;

/// One terminal's share of that callback, and whether an echo still waits.
#[derive(Default)]
pub(crate) struct OutputWakeState {
    drawn: AtomicBool,
    armed: AtomicBool,
    callback: OnceLock<OutputWake>,
}

impl OutputWakeState {
    /// Takes the wake of the window. The first callback stays, because a panel
    /// keeps the viewport that drew it.
    pub(crate) fn attach(&self, wake: &OutputWake) {
        if self.callback.get().is_none() {
            let _ = self.callback.set(Arc::clone(wake));
        }
    }

    /// Whether the frame that just ran drew this terminal. The frame sets it once
    /// per panel, before the keys of that frame reach the PTY.
    pub(crate) fn set_drawn(&self, drawn: bool) {
        self.drawn.store(drawn, Ordering::Relaxed);
    }

    /// A person's input waits for its echo, so the next output asks for a frame.
    /// A panel that the frame does not draw stays asleep, and a pair of atomics
    /// keeps this out of the way of an input path that has no wake of a window to
    /// call.
    pub(crate) fn arm(&self) {
        if !self.drawn.load(Ordering::Relaxed) || self.callback.get().is_none() {
            return;
        }
        self.armed.store(true, Ordering::Release);
    }

    /// The event loop of the terminal read output. The first read after a person
    /// typed asks for the frame that paints it, and later reads wait for it.
    pub(crate) fn wake_on_output(&self) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        if !self.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(wake) = self.callback.get() {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::OutputWakeState;

    fn counting_wake() -> (Arc<AtomicUsize>, Arc<dyn Fn() + Send + Sync>) {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        (count, wake)
    }

    #[test]
    fn output_without_a_key_press_of_a_person_asks_for_no_frame() {
        let state = OutputWakeState::default();
        let (count, wake) = counting_wake();
        state.attach(&wake);
        state.set_drawn(true);

        for _ in 0..1_000 {
            state.wake_on_output();
        }

        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn one_armed_echo_asks_for_one_frame_however_much_output_follows() {
        let state = OutputWakeState::default();
        let (count, wake) = counting_wake();
        state.attach(&wake);
        state.set_drawn(true);

        state.arm();
        for _ in 0..1_000 {
            state.wake_on_output();
        }
        assert_eq!(count.load(Ordering::Relaxed), 1);

        state.arm();
        state.wake_on_output();
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_terminal_without_a_window_wake_arms_nothing() {
        let state = OutputWakeState::default();
        state.set_drawn(true);

        state.arm();
        state.wake_on_output();

        assert!(!state.armed.load(Ordering::Relaxed));
    }

    #[test]
    fn a_terminal_that_no_frame_draws_arms_nothing() {
        let state = OutputWakeState::default();
        let (count, wake) = counting_wake();
        state.attach(&wake);

        state.set_drawn(false);
        state.arm();
        state.wake_on_output();
        assert_eq!(count.load(Ordering::Relaxed), 0, "a hidden panel wakes no window");

        state.set_drawn(true);
        state.arm();
        state.wake_on_output();
        assert_eq!(count.load(Ordering::Relaxed), 1, "a drawn panel wakes its echo");
    }
}
