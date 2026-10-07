//! When the terminals of a cloud are attached or parked. A parked terminal has no
//! local SSH client, PTY or grid; its agent continues in tmux on the worker. The
//! tracker attaches a cloud after it stays in view for a short dwell, so a fast pan
//! across many clouds attaches none of them, and parks it after a grace period out
//! of view, so a short look elsewhere does not cost a new attach.
use std::time::{Duration, Instant};

/// How long a cloud must stay in view or out of view before it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkPolicy {
    pub attach_dwell: Duration,
    pub park_grace: Duration,
}

impl Default for ParkPolicy {
    fn default() -> Self {
        Self {
            attach_dwell: Duration::from_secs(1),
            park_grace: Duration::from_mins(2),
        }
    }
}

/// What a cloud looks like to the user in one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sight {
    /// No terminal of the cloud was drawn.
    Hidden,
    /// A terminal of the cloud was drawn.
    Visible,
    /// The user works in the cloud now: a terminal has focus or fills the window.
    /// A parked cloud attaches at once.
    InUse,
}

/// What the caller must do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkAction {
    Park,
    Attach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Attached { hidden_since: Option<Instant> },
    Parked { seen_since: Option<Instant> },
}

/// The park state of one cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkTracker {
    state: State,
}

impl ParkTracker {
    /// A cloud whose terminals are attached.
    #[must_use]
    pub const fn attached() -> Self {
        Self {
            state: State::Attached { hidden_since: None },
        }
    }

    /// A cloud whose terminals are parked.
    #[must_use]
    pub const fn parked() -> Self {
        Self {
            state: State::Parked { seen_since: None },
        }
    }

    #[must_use]
    pub const fn is_parked(&self) -> bool {
        matches!(self.state, State::Parked { .. })
    }

    /// Records what the user sees at `now`. Returns an action when the cloud
    /// changes; the tracker then already shows the new state.
    pub fn observe(&mut self, sight: Sight, now: Instant, policy: ParkPolicy) -> Option<ParkAction> {
        match (&mut self.state, sight) {
            (State::Attached { hidden_since }, Sight::Hidden) => {
                let since = *hidden_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= policy.park_grace {
                    *self = Self::parked();
                    return Some(ParkAction::Park);
                }
                None
            }
            (State::Attached { hidden_since }, Sight::Visible | Sight::InUse) => {
                *hidden_since = None;
                None
            }
            (State::Parked { seen_since }, Sight::Hidden) => {
                *seen_since = None;
                None
            }
            (State::Parked { seen_since }, Sight::Visible) => {
                let since = *seen_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= policy.attach_dwell {
                    *self = Self::attached();
                    return Some(ParkAction::Attach);
                }
                None
            }
            (State::Parked { .. }, Sight::InUse) => {
                *self = Self::attached();
                Some(ParkAction::Attach)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: ParkPolicy = ParkPolicy {
        attach_dwell: Duration::from_millis(400),
        park_grace: Duration::from_secs(120),
    };

    #[test]
    fn hidden_cloud_parks_only_after_the_whole_grace_period() {
        let start = Instant::now();
        let mut tracker = ParkTracker::attached();
        assert_eq!(tracker.observe(Sight::Hidden, start, POLICY), None);
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_secs(119), POLICY),
            None
        );
        // A short look back restarts the grace period.
        assert_eq!(
            tracker.observe(Sight::Visible, start + Duration::from_secs(119), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_secs(120), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_secs(239), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_secs(240), POLICY),
            Some(ParkAction::Park)
        );
        assert!(tracker.is_parked());
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_secs(900), POLICY),
            None
        );
    }

    #[test]
    fn parked_cloud_attaches_after_a_dwell_and_a_fast_pan_attaches_nothing() {
        let start = Instant::now();
        let mut tracker = ParkTracker::parked();
        assert_eq!(tracker.observe(Sight::Visible, start, POLICY), None);
        assert_eq!(
            tracker.observe(Sight::Visible, start + Duration::from_millis(399), POLICY),
            None
        );
        // The pan moved on before the dwell ended.
        assert_eq!(
            tracker.observe(Sight::Hidden, start + Duration::from_millis(399), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Visible, start + Duration::from_millis(500), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Visible, start + Duration::from_millis(899), POLICY),
            None
        );
        assert_eq!(
            tracker.observe(Sight::Visible, start + Duration::from_millis(900), POLICY),
            Some(ParkAction::Attach)
        );
        assert!(!tracker.is_parked());
    }

    #[test]
    fn use_attaches_at_once_and_holds_the_cloud_attached() {
        let start = Instant::now();
        let mut tracker = ParkTracker::parked();
        assert_eq!(tracker.observe(Sight::InUse, start, POLICY), Some(ParkAction::Attach));
        assert_eq!(
            tracker.observe(Sight::InUse, start + Duration::from_secs(600), POLICY),
            None
        );
        assert!(!tracker.is_parked());
    }
}
