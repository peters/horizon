//! The clock of the server, and the sleep that a refused join takes.
//!
//! The time of a change is the clock of the server, never the clock of a
//! computer. Two waits of the link are visible to a person, so they are values
//! here: the invitation of a join lives for ten minutes, and a wrong code answers
//! after one second. A test replaces the clock and the sleep, so it sees an
//! expired invitation and a slow refusal without waiting for either.

use std::{fmt, thread, time::Duration};

/// The time and the sleep that the server uses.
pub trait Clock: Send + Sync {
    /// The time of the server, in milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;
    /// Waits for `duration`. A test records it instead.
    fn sleep(&self, duration: Duration);
}

/// The clock of the system and its real threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        crate::store::now_millis()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

impl fmt::Debug for dyn Clock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Clock")
            .field("now_millis", &self.now_millis())
            .finish_non_exhaustive()
    }
}
