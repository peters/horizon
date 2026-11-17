//! The output of a panel for a follower that draws it somewhere else.
//!
//! A follower draws one screen of the panel and then the output that follows that
//! screen. The reader of the panel copies the bytes that it hands on into one
//! bounded buffer, and the owner of the panel takes them at the other end. While
//! the outlet copies nothing, a read costs one atomic load: no lock and no copy.
//!
//! The outlet holds only output that follows a screen. It copies from
//! [`ShareOutlet::start`] on, which the owner calls at the instant of a screen,
//! until more than [`SHARE_BUFFER_BYTES`] wait or nobody follows any more. So a
//! take continues the newest screen, or it says that the follower needs a new one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The bytes that wait for a take at most: 1 MiB.
pub const SHARE_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct Shared {
    /// Whether the reader copies. Every write holds the lock of `state`, so the
    /// reader decides with one load and looks again when it holds that lock.
    copying: AtomicBool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    wanted: bool,
    bytes: Vec<u8>,
}

impl Shared {
    /// The lock that a copy or a swap holds, and nothing that waits.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Empties the buffer and gives its memory back, and copies from here or not.
    fn restart(&self, state: &mut State, copying: bool) {
        state.bytes = Vec::new();
        self.copying.store(copying, Ordering::Release);
    }
}

/// The end that the reader of one panel fills.
#[derive(Default)]
pub(crate) struct ShareTap {
    shared: Arc<Shared>,
}

impl ShareTap {
    /// Copies `bytes` of one read while a follower follows a screen.
    pub(crate) fn copy(&self, bytes: &[u8]) {
        if !self.shared.copying.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.shared.state();
        if !self.shared.copying.load(Ordering::Acquire) {
            return;
        }
        if state.bytes.len().saturating_add(bytes.len()) > SHARE_BUFFER_BYTES {
            // Nobody took the output in time. What waits is dropped whole, because
            // a stream with a hole draws a wrong screen.
            self.shared.restart(&mut state, false);
        } else {
            state.bytes.extend_from_slice(bytes);
        }
    }

    /// The other end of this tap.
    pub(crate) fn outlet(&self) -> ShareOutlet {
        ShareOutlet {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// The end where the owner of one panel takes the output for its followers.
#[derive(Clone)]
pub struct ShareOutlet {
    shared: Arc<Shared>,
}

/// What one take of a [`ShareOutlet`] gives.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShareTake {
    /// The output since the take before, in the order of the program.
    pub bytes: Vec<u8>,
    /// Somebody follows and the outlet holds nothing that continues a screen:
    /// more than the bound waited, or no screen started it. The owner sends a
    /// new screen, which starts the outlet again.
    pub overflowed: bool,
}

impl ShareOutlet {
    /// Says whether somebody follows this panel. A change empties the outlet,
    /// and it copies nothing until a screen starts it.
    pub fn set_wanted(&self, wanted: bool) {
        let mut state = self.shared.state();
        if state.wanted != wanted {
            state.wanted = wanted;
            self.shared.restart(&mut state, false);
        }
    }

    /// Empties the outlet and, when somebody follows, copies from the next read
    /// on. The owner calls this at the instant of the screen that it sends.
    pub fn start(&self) {
        let mut state = self.shared.state();
        let wanted = state.wanted;
        self.shared.restart(&mut state, wanted);
    }

    /// The output since the take before, and whether a new screen is due.
    #[must_use]
    pub fn take(&self) -> ShareTake {
        let mut state = self.shared.state();
        ShareTake {
            bytes: std::mem::take(&mut state.bytes),
            overflowed: state.wanted && !self.shared.copying.load(Ordering::Acquire),
        }
    }
}

#[cfg(test)]
mod tests;
