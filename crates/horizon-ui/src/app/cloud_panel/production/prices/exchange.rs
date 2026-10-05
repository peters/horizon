//! Reference rates fetched in the background, independently of provider credentials.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished};
use horizon_core::cloud_runtime::{offers::exchange::Rates, prices::freshness};
use std::{
    sync::mpsc::channel,
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_hours(6);
const REQUEST_MARGIN_MILLIS: i64 = 6_000;

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    rates: Option<Rates>,
    pub error: Option<String>,
    at: Option<Instant>,
    failed_at: Option<Instant>,
    job: Option<Job<Rates>>,
}

impl State {
    pub fn request_for_deadline(&mut self, ctx: &egui::Context, deadline_in_millis: i64) {
        if deadline_in_millis > REQUEST_MARGIN_MILLIS {
            self.request(ctx);
        }
    }
    pub fn request(&mut self, ctx: &egui::Context) {
        if cfg!(test)
            || self.job.is_some()
            || self.fresh().is_some()
            || self.failed_at.is_some_and(|at| at.elapsed() < RETRY_FAILED)
        {
            return;
        }
        let (sender, receiver) = channel();
        self.job = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = Rates::fetch()
                .map(|value| Fetched {
                    value,
                    at: Instant::now(),
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        match finished(&mut self.job) {
            Some(Ok(fetched)) => self.answered(fetched),
            Some(Err(error)) => {
                self.error = Some(error);
                self.failed_at = Some(Instant::now());
            }
            None => {}
        }
    }

    pub fn answered(&mut self, fetched: Fetched<Rates>) {
        self.rates = Some(fetched.value);
        self.at = Some(fetched.at);
        self.error = None;
        self.failed_at = None;
    }

    pub fn fresh(&self) -> Option<&Rates> {
        self.at.filter(|at| at.elapsed() < FRESH)?;
        self.current_rates()
    }

    /// The rates while they are dated for today's comparison, however long ago they came.
    fn current_rates(&self) -> Option<&Rates> {
        self.rates.as_ref().filter(|rates| {
            rates.current(horizon_core::cloud_runtime::offers::exchange::OffsetDateTime::now_utc().date())
        })
    }

    /// The rates while the dialog may compare with them: current, or gone stale while a
    /// background refresh for them runs. Agents' offers use [`Self::fresh`].
    pub fn comparable(&self) -> Option<&Rates> {
        // A manual refresh forgets when the rates came, so they are never superseded here.
        let answer = self.at.map(|at| freshness::Answer::since(at, None));
        freshness::comparable(answer, FRESH, freshness::Fetch::running(self.job.is_some()))
            .then(|| self.current_rates())
            .flatten()
    }

    /// Until rates whose refresh is running stop counting as current, for waking an idle
    /// dialog then.
    pub(super) fn grace_left(&self) -> Option<Duration> {
        self.at
            .filter(|_| self.job.is_some())
            .and_then(|at| freshness::grace_left(at.elapsed(), FRESH))
    }

    pub fn waiting_for_deadline(&self, deadline_in_millis: i64) -> bool {
        self.job.is_some() && deadline_in_millis > ANSWER_MARGIN_MILLIS
    }

    pub fn refresh(&mut self) {
        self.at = None;
        self.failed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_refreshed_and_failed_quotes_cannot_be_used_for_ranking() {
        let rates = Rates {
            date: horizon_core::cloud_runtime::offers::exchange::OffsetDateTime::now_utc()
                .date()
                .to_string(),
            usd_per_unit: std::collections::BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
        };
        let mut state = State::default();
        state.answered(Fetched {
            value: rates.clone(),
            at: Instant::now(),
        });
        assert!(state.fresh().is_some());
        state.at = Instant::now().checked_sub(FRESH + Duration::from_secs(1));
        assert!(state.rates.is_some() && state.fresh().is_none());
        let (sender, receiver) = channel();
        state.job = Some(receiver);
        assert!(state.waiting_for_deadline(6_000));
        assert!(sender.send(Err("unavailable".into())).is_ok());
        state.poll();
        assert!(state.error.is_some() && state.fresh().is_none());
        state.answered(Fetched {
            value: rates,
            at: Instant::now(),
        });
        assert!(state.fresh().is_some());
        state.refresh();
        assert!(state.rates.is_some() && state.fresh().is_none());
    }

    #[test]
    fn a_background_refresh_keeps_the_last_rates_comparable_until_it_answers() {
        let rates = Rates {
            date: horizon_core::cloud_runtime::offers::exchange::OffsetDateTime::now_utc()
                .date()
                .to_string(),
            usd_per_unit: std::collections::BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
        };
        let mut state = State::default();
        assert!(state.comparable().is_none(), "nothing answered yet");
        let Some(answered_at) = Instant::now().checked_sub(FRESH + Duration::from_secs(1)) else {
            return;
        };
        state.answered(Fetched {
            value: rates,
            at: answered_at,
        });
        assert!(state.comparable().is_none() && state.fresh().is_none());
        let (_sender, receiver) = channel();
        state.job = Some(receiver);
        assert!(state.comparable().is_some(), "the refresh keeps the last rates");
        assert!(state.fresh().is_none(), "agents still wait for current rates");
        assert!(
            state.grace_left().is_some_and(|left| left <= freshness::REFRESH_GRACE),
            "the dialog wakes when the grace ends"
        );
        state.refresh();
        assert!(state.comparable().is_none(), "a manual refresh waits for new rates");
    }

    #[test]
    fn pending_rates_leave_time_to_return_native_offers() {
        let (_sender, receiver) = channel();
        let state = State {
            job: Some(receiver),
            ..State::default()
        };
        assert!(state.waiting_for_deadline(6_000));
        assert!(!state.waiting_for_deadline(3_000));
        assert!(!state.waiting_for_deadline(1_001));
        assert!(!State::default().waiting_for_deadline(10_000));
    }
}
