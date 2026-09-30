//! Reference rates fetched in the background, independently of provider credentials.
use super::{Fetched, Job, RETRY_FAILED, finished};
use horizon_core::cloud_runtime::offers::exchange::Rates;
use std::{
    sync::mpsc::channel,
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_hours(6);

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    pub rates: Option<Rates>,
    pub error: Option<String>,
    at: Option<Instant>,
    failed_at: Option<Instant>,
    job: Option<Job<Rates>>,
}

impl State {
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

    pub fn waiting(&self) -> bool {
        self.job.is_some()
    }

    pub fn refresh(&mut self) {
        self.at = None;
        self.failed_at = None;
    }
}
