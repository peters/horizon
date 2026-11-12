//! The clock of one take.
//!
//! Every bound of speech input runs on a clock that a test replaces: a take that
//! stops by itself after 120 seconds and a recorder that ignores `SIGINT` and
//! `SIGTERM` and is killed after 2 seconds are then one test that reaches the bound
//! of the code and no bound of the machine. The code path of the bound is the one
//! that a person reaches.
//!
//! The clock of a test never moves by itself: only the test moves it, and the test
//! moves it only after the real program of the take has shown that it runs. A bound
//! is then reached by the test on its own schedule instead of by a machine that
//! starts a program more slowly than the bound allows.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A monotonic clock that a take times its bounds by.
pub trait Clock: Send + Sync + fmt::Debug {
    /// Milliseconds since this clock started.
    fn now_ms(&self) -> u64;

    /// Wait for `duration`, and let the clock move on by itself.
    ///
    /// The waiting keeps the same length for a take and for a test: a test waits
    /// for real, so a polled take does not spin, and only [`TestClock::advance`]
    /// moves the time that the take reads.
    fn sleep(&self, duration: Duration);
}

/// The clock of this process.
#[derive(Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// A clock that starts now.
    #[must_use]
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// A clock that only a test moves, and that waits the time that a take waits.
///
/// A test that needs "120 seconds passed" or "2 seconds passed after the signal"
/// keeps this clock where it is until the real program of the take has shown that it
/// runs, and then moves the clock by the amount that the test wants. The worker
/// reads the bound on its own schedule, so a bound that the worker computes from an
/// instant of its own — the two seconds of a stop, or the step of 0.3 seconds before
/// `SIGTERM` — is reached in steps: the test that [`Clock::sleep`] returns from
/// moves the clock once more.
///
/// Nothing but a step of a test moves the time of this clock, so a bound of a take
/// can never be reached before a program of a test has started.
#[derive(Debug)]
pub struct TestClock {
    now_ms: AtomicU64,
    waits: AtomicU64,
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClock {
    /// A clock at time 0 that only a test moves.
    #[must_use]
    pub fn new() -> Self {
        Self {
            now_ms: AtomicU64::new(0),
            waits: AtomicU64::new(0),
        }
    }

    /// Move the clock forward by `duration`.
    pub fn advance(&self, duration: Duration) {
        self.advance_ms(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
    }

    /// Move the clock forward by `millis` milliseconds.
    pub fn advance_ms(&self, millis: u64) {
        self.now_ms.fetch_add(millis, Ordering::SeqCst);
    }

    /// How often a take waited on this clock.
    #[must_use]
    pub fn waits(&self) -> u64 {
        self.waits.load(Ordering::SeqCst)
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) {
        // The wait is real, so a take that polls uses no core of the machine, and
        // the time of the clock stays where the test put it.
        std::thread::sleep(duration);
        self.waits.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Clock, TestClock};

    #[test]
    fn only_a_step_of_a_test_moves_the_time_of_a_clock_of_a_test() {
        let clock = TestClock::new();
        assert_eq!(clock.now_ms(), 0);
        clock.sleep(Duration::from_millis(10));
        assert_eq!(clock.now_ms(), 0, "a wait of a take moves no time of a test");
        assert_eq!(clock.waits(), 1);
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.now_ms(), 3000);
        assert_eq!(clock.waits(), 1);
        clock.advance_ms(10);
        assert_eq!(clock.now_ms(), 3010);
    }
}
