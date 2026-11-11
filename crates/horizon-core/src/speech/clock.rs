//! The clock of one take.
//!
//! Every bound of speech input runs on a clock that a test replaces: a take that
//! stops by itself after 120 seconds and a recorder that ignores `SIGINT` and is
//! killed after 2 seconds are then one test that takes no second of wall time. The
//! code path of the bound is the one that a person reaches.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A monotonic clock that a take times its bounds by.
pub trait Clock: Send + Sync + fmt::Debug {
    /// Milliseconds since this clock started.
    fn now_ms(&self) -> u64;

    /// Wait for `duration`. The clock of a test moves its own time instead, so the
    /// waiting of one take costs no wall time.
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

/// A clock that a test moves by hand, and that waits no wall time.
///
/// The waits of a take either move the time of the test, which is what a bound
/// needs, or leave it as the test set it, which is what a test of a short take
/// needs: nothing but the test moves the clock, so one take is exactly as long as
/// the test says.
#[derive(Debug)]
pub struct TestClock {
    now_ms: AtomicU64,
    waits: AtomicU64,
    sleep_moves: bool,
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClock {
    /// A clock at time 0 whose waits move its time, so a bound of a take is reached
    /// without wall time.
    #[must_use]
    pub fn new() -> Self {
        Self {
            now_ms: AtomicU64::new(0),
            waits: AtomicU64::new(0),
            sleep_moves: true,
        }
    }

    /// A clock at time 0 that only a test moves.
    #[must_use]
    pub fn frozen() -> Self {
        Self {
            sleep_moves: false,
            ..Self::new()
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
        if self.sleep_moves {
            self.advance_ms(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX).max(1));
        }
        self.waits.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Clock, TestClock};

    #[test]
    fn a_clock_of_a_test_moves_by_its_own_waits_or_only_by_a_step() {
        let clock = TestClock::new();
        assert_eq!(clock.now_ms(), 0);
        clock.sleep(Duration::from_millis(10));
        assert_eq!(clock.now_ms(), 10);
        assert_eq!(clock.waits(), 1);
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.now_ms(), 3010);
        assert_eq!(clock.waits(), 1);

        let frozen = TestClock::frozen();
        frozen.sleep(Duration::from_secs(10));
        assert_eq!(frozen.now_ms(), 0, "only the test moves a frozen clock");
        assert_eq!(frozen.waits(), 1);
        frozen.advance(Duration::from_millis(5));
        assert_eq!(frozen.now_ms(), 5);
    }
}
