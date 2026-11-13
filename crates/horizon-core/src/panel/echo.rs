//! The clock that the opt-in line of a key press is built from.
//!
//! A panel owns a terminal, and these calls are the part of it that the frame and
//! the reader thread of that terminal share: the time that a frame took a key
//! event, the write of its bytes, and the paint of the grid that echoes it.
//! Nothing here changes the grid, so it sits beside the lifecycle of the panel
//! instead of inside it.

use std::time::Instant;

use super::Panel;

impl Panel {
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
