//! When a viewer reaches its desktop again by itself after a lost link, and how
//! long it waits before the next attempt.
//!
//! The delays belong to the caller, because the machine terminals of the
//! application already hold one table of them and a viewer of the same link must
//! not wait by another rule. [`Reconnect::default`] retries nothing, so a caller
//! that wants no automatic attempt keeps the single connection of earlier
//! versions.

use std::time::Duration;

use super::Error;

/// How a session tries a lost connection again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reconnect {
    /// The delay before attempt `n`, counted from 1. An empty list retries
    /// nothing.
    pub delays: Vec<Duration>,
    /// The delay of every attempt after the ones of `delays`.
    pub repeat: Duration,
    /// A connection that lasted at least this long starts the delays again.
    pub reset_after: Duration,
}

/// What the session does after a connection ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterLoss {
    /// Wait `after`, then connect again as `attempt`.
    Retry { attempt: u32, after: Duration },
    /// Report the failure and stay stopped.
    Stay,
}

impl Reconnect {
    /// The delay before `attempt`, counted from 1, or None when this policy does
    /// not retry.
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Option<Duration> {
        if self.delays.is_empty() {
            return None;
        }
        usize::try_from(attempt)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .map(|index| self.delays.get(index).copied().unwrap_or(self.repeat))
    }

    /// The delay before the next attempt after a connection that was up for
    /// `connected_for` and followed `attempts` earlier losses.
    #[must_use]
    pub fn next_attempt(&self, attempts: u32, connected_for: Option<Duration>) -> Option<(u32, Duration)> {
        let attempts = match connected_for {
            Some(up) if up >= self.reset_after => 0,
            _ => attempts,
        };
        let attempt = attempts.saturating_add(1);
        self.delay(attempt).map(|after| (attempt, after))
    }
}

/// What the session does after the connection of the previous attempt ended.
///
/// A refusal of the host key is a decision that only a person can make, so it is
/// never retried. A cancelled connection is not a loss. Every other failure is a
/// link that may come back, and the policy of the caller decides how often the
/// session tries.
pub(crate) fn after_loss(
    error: &Error,
    policy: &Reconnect,
    attempts: u32,
    connected_for: Option<Duration>,
) -> AfterLoss {
    if error.host_key_failure().is_some() {
        return AfterLoss::Stay;
    }
    match policy.next_attempt(attempts, connected_for) {
        Some((attempt, after)) => AfterLoss::Retry { attempt, after },
        None => AfterLoss::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::host_key::{HostKeyFailure, HostKeyRefusal};

    fn policy() -> Reconnect {
        Reconnect {
            delays: vec![
                Duration::from_secs(3),
                Duration::from_secs(6),
                Duration::from_secs(12),
                Duration::from_secs(24),
            ],
            repeat: Duration::from_secs(30),
            reset_after: Duration::from_secs(10),
        }
    }

    #[test]
    fn the_delays_climb_and_repeat() {
        assert_eq!(
            (1..=6)
                .map(|attempt| policy().delay(attempt).map(|delay| delay.as_secs()))
                .collect::<Vec<_>>(),
            vec![Some(3), Some(6), Some(12), Some(24), Some(30), Some(30)]
        );
        assert_eq!(policy().delay(0), None, "attempts are counted from one");
        assert_eq!(Reconnect::default().delay(1), None, "the default retries nothing");
    }

    #[test]
    fn a_connection_that_lasted_resets_the_delays() {
        assert_eq!(
            policy().next_attempt(4, Some(Duration::from_secs(10))),
            Some((1, Duration::from_secs(3)))
        );
        assert_eq!(
            policy().next_attempt(4, Some(Duration::from_millis(9_999))),
            Some((5, Duration::from_secs(30))),
            "a short connection continues where it was"
        );
        assert_eq!(
            policy().next_attempt(4, None),
            Some((5, Duration::from_secs(30))),
            "an attempt that never connected is a loss"
        );
    }

    #[test]
    fn a_host_key_refusal_is_never_retried() {
        for refusal in [HostKeyRefusal::NotTrusted, HostKeyRefusal::Changed] {
            let error = Error::Tunnel {
                message: "Host key verification failed.".to_string(),
                host_key: Some(Box::new(HostKeyFailure {
                    refusal,
                    host: "lab".into(),
                    port: Some(22),
                    observed: None,
                    unobserved: None,
                    resolved: None,
                })),
            };
            assert_eq!(after_loss(&error, &policy(), 0, None), AfterLoss::Stay, "{refusal:?}");
        }
        assert_eq!(
            after_loss(&Error::Timeout, &policy(), 0, None),
            AfterLoss::Retry {
                attempt: 1,
                after: Duration::from_secs(3)
            },
            "a stall is a link that may come back"
        );
    }
}
