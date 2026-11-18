//! The sign of life of one live link, on both sides of it: a heartbeat frame
//! while the link has nothing else to send, and the end of a link that read
//! nothing for its bound.
//!
//! A computer that goes away without closing its transport stays in the list of
//! the other computers until that transport fails, which on TCP can take very
//! long. Each side therefore sends a small frame every [`Limits::heartbeat`]
//! while the link is idle, and each side ends the link when the other one sent
//! nothing for [`Limits::silence`]. The transport is closed after the error
//! frame, so a read that waits ends too.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The bounds of the sign of life of a live link. A test shortens them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// How long one look at the link waits: the smallest of the steps below.
    pub(crate) tick: Duration,
    /// How long an idle link waits between two heartbeats.
    pub(crate) heartbeat: Duration,
    /// How long a link may read nothing before it ends.
    pub(crate) silence: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(1),
            heartbeat: Duration::from_secs(10),
            silence: Duration::from_secs(30),
        }
    }
}

/// When this side of one live link last read a frame and last sent one.
#[derive(Debug)]
pub(crate) struct Alive {
    limits: Limits,
    times: Mutex<Times>,
}

#[derive(Clone, Copy, Debug)]
struct Times {
    read: Instant,
    sent: Instant,
}

impl Alive {
    pub(crate) fn new(limits: Limits) -> Self {
        let now = Instant::now();
        let times = Times { read: now, sent: now };
        Self {
            limits,
            times: Mutex::new(times),
        }
    }

    fn times(&self) -> MutexGuard<'_, Times> {
        self.times.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One frame of the other side arrived.
    pub(crate) fn read(&self) {
        self.times().read = Instant::now();
    }

    /// One frame of this side went to the link.
    pub(crate) fn sent(&self) {
        self.times().sent = Instant::now();
    }

    /// How long one look at the link waits.
    pub(crate) fn tick(&self) -> Duration {
        self.limits.tick
    }

    /// Whether a heartbeat is due, because this side sent nothing for the bound.
    pub(crate) fn beat_due(&self) -> bool {
        self.times().sent.elapsed() >= self.limits.heartbeat
    }

    /// Whether the other side sent nothing for the bound of a silent link.
    pub(crate) fn quiet(&self) -> bool {
        self.times().read.elapsed() >= self.limits.silence
    }

    /// The reason of a link that ends because its server went quiet, for the
    /// person who reads the state of the link.
    pub(crate) fn quiet_reason(&self) -> String {
        let seconds = self.limits.silence.as_secs();
        format!("the server sent no frame for {seconds} seconds")
    }
}

impl Default for Alive {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{Alive, Limits};
    use std::thread;
    use std::time::Duration;

    /// Bounds short enough for a test, and long enough that one sleep of a busy
    /// machine does not pass two of them.
    fn quick() -> Limits {
        Limits {
            tick: Duration::from_millis(5),
            heartbeat: Duration::from_millis(40),
            silence: Duration::from_millis(120),
        }
    }

    #[test]
    fn a_link_is_quiet_only_after_its_bound_and_a_heartbeat_only_after_its_period() {
        let alive = Alive::new(quick());
        assert!(!alive.quiet(), "a new link read a frame now");
        assert!(!alive.beat_due(), "a new link sent a frame now");

        thread::sleep(Duration::from_millis(60));
        assert!(alive.beat_due(), "no frame went out for the period");
        assert!(!alive.quiet(), "the silence bound has not passed");
        alive.sent();
        alive.read();
        assert!(!alive.beat_due(), "a frame went out now");

        thread::sleep(Duration::from_millis(70));
        assert!(!alive.quiet(), "the newest frame of the other side is inside the bound");
        assert!(alive.beat_due(), "the period of the heartbeat passed");
        alive.read();

        thread::sleep(Duration::from_millis(70));
        assert!(!alive.quiet(), "a frame of the other side arrived 70 ms ago");
        thread::sleep(Duration::from_millis(60));
        assert!(alive.quiet(), "the other side sent nothing for the bound");
        assert_eq!(
            Alive::default().quiet_reason(),
            "the server sent no frame for 30 seconds"
        );
    }
}
