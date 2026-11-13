//! The hooks that a frame uses to show and to time the echo of a person's key press.
//!
//! A panel owns a terminal, and these four calls are the part of it that the
//! frame and the reader thread share: the wake that brings the frame which paints
//! an echo, and the clock that the opt-in latency line of one press is built from.
//! Nothing here changes the grid, so it sits beside the lifecycle of the panel
//! instead of inside it.

use std::time::Instant;

use crate::terminal::OutputWake;

use super::Panel;

impl Panel {
    /// Takes the callback that asks the window for the frame which shows new
    /// output of this panel's terminal.
    pub fn attach_output_wake(&self, wake: &OutputWake) {
        if let Some(terminal) = self.terminal() {
            terminal.attach_output_wake(wake);
        }
    }

    /// A person typed into this panel, so its echo asks for the frame that shows
    /// it instead of waiting for the next idle poll of the window.
    pub fn arm_output_wake(&self) {
        if let Some(terminal) = self.terminal() {
            terminal.arm_output_wake();
        }
    }

    /// Whether the frame that just ran drew this panel's grid. A panel that no
    /// frame draws arms no wake for its output.
    pub fn note_drawn(&self, drawn: bool) {
        if let Some(terminal) = self.terminal() {
            terminal.note_drawn(drawn);
        }
    }

    /// The time that this frame took a key event of this panel's terminal, while
    /// its latency lines are on.
    #[must_use]
    pub fn press_clock(&self) -> Option<Instant> {
        self.terminal()?.press_clock()
    }

    /// Records one key press that this frame wrote, for the latency lines.
    pub fn note_key_press(&self, event_at: Instant, bytes: usize) {
        if let Some(terminal) = self.terminal() {
            terminal.note_key_press(event_at, bytes);
        }
    }

    /// Records that this frame painted the grid of this panel's terminal.
    pub fn note_output_painted(&self) {
        if let Some(terminal) = self.terminal() {
            terminal.note_output_painted();
        }
    }
}
