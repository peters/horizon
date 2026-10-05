//! When prices on show still count as current for comparing workers.
//!
//! Prices are fetched again in the background once they go stale. Until that refresh
//! answers, the comparison it will replace stays as it was for a bounded time, so the
//! New cloud dialog does not switch between complete and incomplete, and move its
//! rows, on every refresh. Answers that were superseded on purpose, for example by a
//! manual refresh or a credential change, never count. Callers report failed fetches
//! separately.
use std::time::{Duration, Instant};

/// How long past going stale prices still count as current while a refresh for them
/// runs. A refresh that takes longer leaves the comparison incomplete.
pub const REFRESH_GRACE: Duration = Duration::from_secs(30);

/// A provider's last answer, as far as its currency is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// How long ago the provider answered.
    pub age: Duration,
    /// Newer prices were asked for on purpose since, for example by a manual refresh
    /// or a credential change. Providers that also supersede on a failed fetch say so.
    pub superseded: bool,
}

impl Answer {
    /// The answer a provider gave at `answered`, superseded when newer prices were
    /// asked for on purpose at `asked_again`, after it.
    #[must_use]
    pub fn since(answered: Instant, asked_again: Option<Instant>) -> Self {
        Self {
            age: answered.elapsed(),
            superseded: asked_again.is_some_and(|asked| answered < asked),
        }
    }
}

/// Whether a request for newer prices is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    Idle,
    Refreshing,
}

impl Fetch {
    /// [`Self::Refreshing`] while a request is `running`, otherwise [`Self::Idle`].
    #[must_use]
    pub const fn running(running: bool) -> Self {
        if running { Self::Refreshing } else { Self::Idle }
    }
}

/// Whether prices from `answer` count as current for a comparison: younger than
/// `fresh_for`, or stale for less than [`REFRESH_GRACE`] while a background refresh
/// runs. No answer yet, as on the first load, is never current.
#[must_use]
pub fn comparable(answer: Option<Answer>, fresh_for: Duration, fetch: Fetch) -> bool {
    let Some(answer) = answer else {
        return false;
    };
    if answer.superseded {
        return false;
    }
    let limit = match fetch {
        Fetch::Idle => fresh_for,
        Fetch::Refreshing => fresh_for.saturating_add(REFRESH_GRACE),
    };
    answer.age < limit
}

/// How long until an answer `age` old stops counting as current while a refresh for
/// it runs, for waking an idle dialog then; `None` once it no longer counts.
#[must_use]
pub fn grace_left(age: Duration, fresh_for: Duration) -> Option<Duration> {
    Some(fresh_for.saturating_add(REFRESH_GRACE).saturating_sub(age)).filter(|left| !left.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: Duration = Duration::from_secs(15);

    fn answered(seconds: u64) -> Answer {
        Answer {
            age: Duration::from_secs(seconds),
            superseded: false,
        }
    }

    #[test]
    fn the_first_load_is_never_current() {
        assert!(!comparable(None, FRESH, Fetch::Idle));
        assert!(!comparable(None, FRESH, Fetch::Refreshing));
    }

    #[test]
    fn a_young_answer_is_current_whether_or_not_a_refresh_runs() {
        assert!(comparable(Some(answered(3)), FRESH, Fetch::Idle));
        assert!(comparable(Some(answered(3)), FRESH, Fetch::Refreshing));
    }

    #[test]
    fn a_background_refresh_keeps_the_last_answer_current() {
        assert!(comparable(Some(answered(15)), FRESH, Fetch::Refreshing));
        assert!(comparable(Some(answered(44)), FRESH, Fetch::Refreshing));
    }

    #[test]
    fn a_stale_answer_without_a_refresh_is_not_current() {
        assert!(!comparable(Some(answered(15)), FRESH, Fetch::Idle));
        assert!(!comparable(Some(answered(3_600)), FRESH, Fetch::Idle));
    }

    #[test]
    fn a_refresh_that_takes_too_long_ends_the_grace() {
        assert!(!comparable(Some(answered(45)), FRESH, Fetch::Refreshing));
        assert!(!comparable(Some(answered(3_600)), FRESH, Fetch::Refreshing));
    }

    #[test]
    fn a_superseded_answer_is_never_current() {
        let superseded = Some(Answer {
            age: Duration::from_secs(1),
            superseded: true,
        });
        assert!(!comparable(superseded, FRESH, Fetch::Idle));
        assert!(!comparable(superseded, FRESH, Fetch::Refreshing));
    }

    #[test]
    fn an_answer_is_superseded_only_by_a_later_request() {
        let answered = Instant::now();
        assert!(!Answer::since(answered, None).superseded);
        if let Some(earlier) = answered.checked_sub(Duration::from_secs(1)) {
            assert!(!Answer::since(answered, Some(earlier)).superseded);
        }
        let later = answered + Duration::from_secs(1);
        assert!(Answer::since(answered, Some(later)).superseded);
    }

    #[test]
    fn a_running_request_is_a_refresh() {
        assert_eq!(Fetch::running(true), Fetch::Refreshing);
        assert_eq!(Fetch::running(false), Fetch::Idle);
    }

    #[test]
    fn the_grace_left_counts_down_to_none() {
        assert_eq!(grace_left(Duration::ZERO, FRESH), Some(FRESH + REFRESH_GRACE));
        assert_eq!(grace_left(Duration::from_secs(40), FRESH), Some(Duration::from_secs(5)));
        assert_eq!(grace_left(FRESH + REFRESH_GRACE, FRESH), None);
        assert_eq!(grace_left(Duration::from_hours(1), FRESH), None);
    }
}
