//! Reference rates fetched in the background, independently of provider credentials.
use super::{Fetched, Job, RETRY_FAILED, finished};
use horizon_core::cloud_runtime::offers::exchange::Rates;
use std::{
    sync::mpsc::channel,
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_hours(6);
const REQUEST_MARGIN_MILLIS: i64 = 6_000;
const ANSWER_MARGIN_MILLIS: i64 = 3_000;

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    pub rates: Option<Rates>,
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
            || self.at.is_some_and(|at| at.elapsed() < FRESH)
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
            Some(Ok(fetched)) => {
                self.rates = Some(fetched.value);
                self.at = Some(fetched.at);
                self.error = None;
                self.failed_at = None;
            }
            Some(Err(error)) => {
                self.error = Some(error);
                self.failed_at = Some(Instant::now());
            }
            None => {}
        }
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
