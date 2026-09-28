//! Notices when this computer slept or moved to another network while it shared one, so the
//! card can pause sharing and, on a new network, ask the owner again.
use horizon_core::cloud_runtime::local_network::Network;
use std::time::{Duration, Instant, SystemTime};

/// How far the wall clock may run ahead of the monotonic clock between two checks before it
/// counts as sleep. The monotonic clock stops while the computer is suspended on Linux and
/// macOS; the wall clock does not. Clock adjustments smaller than this are ignored. On Windows
/// the monotonic clock keeps running through suspend, so sleep is not noticed there; a move to
/// another network after waking still is.
const SLEEP_GAP: Duration = Duration::from_secs(15);
/// How often a running bridge compares the current network with the one it shares.
pub(super) const NETWORK_CHECK: Duration = Duration::from_secs(2);

/// The two clocks, read together.
#[derive(Clone, Copy, Debug)]
pub(super) struct Clock {
    pub(super) monotonic: Instant,
    pub(super) wall: SystemTime,
}

impl Clock {
    pub(super) fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

/// When sharing last looked at the clocks and at the network.
#[derive(Debug)]
pub(in super::super) struct Watch {
    seen: Clock,
    checked: Instant,
}

impl Default for Watch {
    fn default() -> Self {
        Self::new(Clock::now())
    }
}

impl Watch {
    pub(super) fn new(now: Clock) -> Self {
        Self {
            seen: now,
            checked: now.monotonic,
        }
    }

    /// Whether the computer slept since the last call, judged by the wall clock running ahead
    /// of the monotonic clock.
    pub(super) fn slept(&mut self, now: Clock) -> bool {
        let monotonic = now.monotonic.saturating_duration_since(self.seen.monotonic);
        let wall = now.wall.duration_since(self.seen.wall).unwrap_or_default();
        self.seen = now;
        wall > monotonic + SLEEP_GAP
    }

    /// Whether it is time to compare the current network with the shared one.
    pub(super) fn check_due(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.checked) < NETWORK_CHECK {
            return false;
        }
        self.checked = now;
        true
    }
}

/// Whether `current` is still the network sharing started on. Sharing without a recorded
/// network, which only tests start, never counts as moved.
pub(super) fn same_network(shared: Option<&Network>, current: Option<&Network>) -> bool {
    shared.is_none_or(|shared| current == Some(shared))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(monotonic: Instant, wall: SystemTime) -> Clock {
        Clock { monotonic, wall }
    }

    #[test]
    fn sleep_is_the_wall_clock_running_ahead_of_the_monotonic_one() {
        let (start, wall) = (Instant::now(), SystemTime::now());
        let mut watch = Watch::new(clock(start, wall));
        let minute = Duration::from_secs(60);
        assert!(!watch.slept(clock(start + minute, wall + minute)), "an awake minute");
        assert!(
            watch.slept(clock(start + minute + Duration::from_secs(1), wall + minute * 11)),
            "ten minutes of wall time in one second of monotonic time"
        );
        assert!(
            !watch.slept(clock(
                start + minute * 2,
                wall + minute * 11 + minute + Duration::from_secs(5)
            )),
            "a small clock adjustment is not sleep"
        );
        assert!(
            !watch.slept(clock(start + minute * 3, wall)),
            "a wall clock set back is not sleep"
        );
    }

    #[test]
    fn the_network_is_compared_every_few_seconds() {
        let start = Instant::now();
        let mut watch = Watch::new(clock(start, SystemTime::now()));
        assert!(!watch.check_due(start + Duration::from_secs(1)));
        assert!(watch.check_due(start + NETWORK_CHECK));
        assert!(!watch.check_due(start + NETWORK_CHECK + Duration::from_secs(1)));
    }

    #[test]
    fn a_move_is_a_different_network_or_none() {
        let home = Network::new(
            "192.168.1.0/24".parse().unwrap(),
            "192.168.1.20".parse().unwrap(),
            "wlan0",
        );
        let office = Network::new("10.0.0.0/24".parse().unwrap(), "10.0.0.7".parse().unwrap(), "wlan0");
        assert!(same_network(Some(&home), Some(&home)));
        assert!(!same_network(Some(&home), Some(&office)));
        assert!(!same_network(Some(&home), None), "no shareable network now");
        assert!(same_network(None, None));
    }
}
