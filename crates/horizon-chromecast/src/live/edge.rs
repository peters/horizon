//! Decides how fast progressive playback should run so the receiver stays near
//! the live edge without being left on a failed attempt.
//!
//! A media player cannot be told to present one frame at a clock time. It
//! holds a buffer. The previous controller only sped up once that buffer
//! passed one second, and it always stopped at 0.4 s, so anything in between
//! stayed there. This one trims as soon as the lag passes the current target.
//! It starts at that 0.4 s target, which is the point a Google TV has played
//! smoothly, and after a calm stretch it tries 50 ms closer, down to 0.15 s.
//! Half a second of buffering while it is already near the edge steps the
//! target back once. That episode does not step again until playback resumes,
//! and the session does not try the failed level again.

use std::time::Duration;

/// Lag left in place until playback has been calm. A Google TV played smoothly
/// here; closer than this is attempted only after that.
pub(crate) const PROVEN_LAG: f64 = 0.40;
/// Closest target. Near one tenth of a second, with room for one video frame
/// and the AAC frame a player has to hold.
pub(crate) const LIVE_EDGE: f64 = 0.15;
const STEP: f64 = 0.05;
/// Speed up once the lag is this far above the target.
const ACT: f64 = 0.05;
/// Return to normal speed once the lag is this close to the target.
const ARRIVED: f64 = 0.02;
/// Above this the startup buffer is eaten quickly. Below it, creep.
const FAR: f64 = 1.0;
const FAR_RATE: f64 = 1.5;
const NEAR_RATE: f64 = 1.08;
const CREEP_AFTER: Duration = Duration::from_secs(4);
/// Longer than the brief BUFFERING reports a TV sends between PLAYING updates.
const STALL: Duration = Duration::from_millis(500);
/// Buffering with more lag than this is not the target being too low.
const STALL_LAG: f64 = 0.80;
const MAX_LAG: f64 = 30.0;

/// Where the controller wants playback, and the rate it believes is applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Edge {
    pub target: f64,
    /// Lowest target this session may still try.
    pub floor: f64,
    pub rate: f64,
    pub settled_for: Duration,
    /// The open buffering episode has already stepped the target back.
    pub backed_off: bool,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            target: PROVEN_LAG,
            floor: LIVE_EDGE,
            rate: 1.0,
            settled_for: Duration::ZERO,
            backed_off: false,
        }
    }
}

/// One observation from the receiver.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    /// Newest pushed frame minus the receiver's `currentTime`, in seconds.
    pub lag: f64,
    /// How long the current BUFFERING report has lasted, if one is open.
    pub buffering_for: Option<Duration>,
    /// Time since the previous sample.
    pub elapsed: Duration,
}

impl Edge {
    /// Next target and playback rate for `sample`.
    #[must_use]
    pub(crate) fn step(mut self, sample: Sample) -> Self {
        if !sample.lag.is_finite() || sample.lag > MAX_LAG {
            // Do not move the target on a sample we cannot trust. Do not stay
            // fast either: there is no later deadline that restores normal speed.
            self.rate = 1.0;
            self.settled_for = Duration::ZERO;
            return self;
        }
        // A resumed picture may try the next step. The same episode may not.
        if sample.buffering_for.is_none() {
            self.backed_off = false;
        }
        if self.stalled(sample) {
            if !self.backed_off {
                let raised = (self.target + STEP).min(PROVEN_LAG);
                self.floor = raised;
                self.target = raised;
                self.backed_off = true;
            }
            self.rate = 1.0;
            self.settled_for = Duration::ZERO;
            return self;
        }
        if sample.lag < 0.0 {
            self.rate = 1.0;
            self.settled_for = Duration::ZERO;
            return self;
        }
        // A buffering report is not calm playback, even when it is shorter
        // than the stall that steps the target back.
        if sample.buffering_for.is_some() {
            self.settled_for = Duration::ZERO;
        } else if self.rate <= 1.0 && sample.lag <= self.target + ACT {
            self.settled_for = self.settled_for.saturating_add(sample.elapsed);
            if self.settled_for >= CREEP_AFTER && self.target > self.floor + f64::EPSILON {
                self.target = (self.target - STEP).max(self.floor);
                self.settled_for = Duration::ZERO;
            }
        } else {
            self.settled_for = Duration::ZERO;
        }
        self.rate = self.wanted(sample.lag);
        self
    }

    /// Only a target we have already lowered can be too close. Buffering while
    /// still at the proven lag is the TV's ordinary report, or a startup
    /// buffer, and must not stop the attempt to go closer.
    fn stalled(self, sample: Sample) -> bool {
        sample.buffering_for.is_some_and(|since| since >= STALL)
            && sample.lag < STALL_LAG
            && self.target + f64::EPSILON < PROVEN_LAG
    }

    fn wanted(self, lag: f64) -> f64 {
        if lag > FAR {
            FAR_RATE
        } else if lag > self.target + ACT {
            NEAR_RATE
        } else if lag <= self.target + ARRIVED {
            1.0
        } else {
            self.rate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(lag: f64, buffering_for: Option<Duration>, elapsed: Duration) -> Sample {
        Sample {
            lag,
            buffering_for,
            elapsed,
        }
    }

    fn at(target: f64, floor: f64, rate: f64, settled_for: Duration) -> Edge {
        Edge {
            target,
            floor,
            rate,
            settled_for,
            backed_off: false,
        }
    }

    #[test]
    fn lag_between_the_old_threshold_and_the_target_now_speeds_up() {
        let next = Edge::default().step(sample(0.65, None, Duration::from_millis(400)));
        assert!((next.target - PROVEN_LAG).abs() < f64::EPSILON);
        assert!((next.rate - NEAR_RATE).abs() < f64::EPSILON);
        let far = Edge::default().step(sample(2.4, None, Duration::from_millis(400)));
        assert!((far.rate - FAR_RATE).abs() < f64::EPSILON);
    }

    #[test]
    fn normal_speed_returns_at_the_target_and_holds_inside_the_band() {
        let speeding = at(PROVEN_LAG, LIVE_EDGE, NEAR_RATE, Duration::ZERO);
        let arrived = speeding.step(sample(0.41, None, Duration::from_millis(400)));
        assert!((arrived.rate - 1.0).abs() < f64::EPSILON);
        let inside = speeding.step(sample(0.44, None, Duration::from_millis(400)));
        assert!((inside.rate - NEAR_RATE).abs() < f64::EPSILON);
    }

    #[test]
    fn a_calm_stretch_steps_toward_the_live_edge_and_then_trims() {
        let calm = at(
            PROVEN_LAG,
            LIVE_EDGE,
            1.0,
            CREEP_AFTER.saturating_sub(Duration::from_millis(400)),
        );
        let next = calm.step(sample(0.41, None, Duration::from_millis(400)));
        assert!((next.target - 0.35).abs() < 1e-9);
        assert!((next.rate - NEAR_RATE).abs() < f64::EPSILON);
        let edge = at(LIVE_EDGE, LIVE_EDGE, 1.0, CREEP_AFTER);
        let held = edge.step(sample(0.16, None, Duration::from_millis(400)));
        assert!((held.target - LIVE_EDGE).abs() < f64::EPSILON);
        assert!((held.rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn buffering_near_the_edge_steps_back_and_does_not_retry() {
        let trying = at(0.25, LIVE_EDGE, NEAR_RATE, Duration::from_secs(3));
        let next = trying.step(sample(0.22, Some(STALL), Duration::from_millis(400)));
        assert!((next.target - 0.30).abs() < 1e-9);
        assert!((next.floor - 0.30).abs() < f64::EPSILON);
        assert!((next.rate - 1.0).abs() < f64::EPSILON);
        let later = next.step(sample(0.31, None, CREEP_AFTER));
        assert!((later.target - 0.30).abs() < 1e-9);
    }

    #[test]
    fn one_buffering_episode_steps_back_once() {
        let trying = at(LIVE_EDGE, LIVE_EDGE, 1.0, Duration::ZERO);
        let once = trying.step(sample(0.16, Some(STALL), Duration::from_millis(400)));
        assert!((once.target - 0.20).abs() < 1e-9);
        assert!(once.backed_off);
        let still = once.step(sample(0.16, Some(Duration::from_secs(3)), Duration::from_millis(400)));
        assert!((still.target - 0.20).abs() < 1e-9);
        assert!((still.floor - 0.20).abs() < 1e-9);
        let resumed = still.step(sample(0.21, None, Duration::from_millis(400)));
        assert!(!resumed.backed_off);
        let again = resumed.step(sample(0.20, Some(STALL), Duration::from_millis(400)));
        assert!((again.target - 0.25).abs() < 1e-9);
    }

    #[test]
    fn buffering_does_not_count_as_calm() {
        let calm = at(
            PROVEN_LAG,
            LIVE_EDGE,
            1.0,
            CREEP_AFTER.saturating_sub(Duration::from_millis(400)),
        );
        let next = calm.step(sample(
            0.41,
            Some(Duration::from_millis(200)),
            Duration::from_millis(400),
        ));
        assert!((next.target - PROVEN_LAG).abs() < 1e-9);
        assert_eq!(next.settled_for, Duration::ZERO);
    }

    #[test]
    fn a_startup_buffer_and_a_blip_at_the_proven_lag_do_not_give_up() {
        let eating = at(PROVEN_LAG, LIVE_EDGE, FAR_RATE, Duration::ZERO);
        let startup = eating.step(sample(2.0, Some(Duration::from_secs(2)), Duration::from_millis(400)));
        assert!((startup.target - PROVEN_LAG).abs() < f64::EPSILON);
        assert!((startup.floor - LIVE_EDGE).abs() < f64::EPSILON);
        let trimming = at(PROVEN_LAG, LIVE_EDGE, NEAR_RATE, Duration::ZERO);
        let during = trimming.step(sample(0.50, Some(STALL), Duration::from_millis(400)));
        assert!((during.target - PROVEN_LAG).abs() < f64::EPSILON);
        assert!((during.floor - LIVE_EDGE).abs() < f64::EPSILON);
        let blip = Edge::default().step(sample(
            0.45,
            Some(Duration::from_millis(200)),
            Duration::from_millis(400),
        ));
        assert!((blip.floor - LIVE_EDGE).abs() < f64::EPSILON);
        assert!((blip.rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_clock_that_runs_ahead_or_reports_nonsense_stops_speeding() {
        let speeding = at(0.30, LIVE_EDGE, NEAR_RATE, Duration::ZERO);
        let ahead = speeding.step(sample(-0.05, None, Duration::from_millis(400)));
        assert!((ahead.rate - 1.0).abs() < f64::EPSILON);
        assert!((ahead.target - 0.30).abs() < f64::EPSILON);
        let mut stopped = speeding;
        stopped.rate = 1.0;
        let nonsense = speeding.step(sample(f64::NAN, None, Duration::from_millis(400)));
        assert_eq!(nonsense, stopped);
        let huge = speeding.step(sample(MAX_LAG + 1.0, None, Duration::from_millis(400)));
        assert_eq!(huge, stopped);
    }
}
