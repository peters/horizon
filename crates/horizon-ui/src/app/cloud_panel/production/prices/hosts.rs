//! What each saved Docker host can take now, read in the background for New cloud,
//! Cloud settings and agents' offer requests. Nothing is reserved.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished, spawn};
use horizon_core::cloud_runtime::docker_host::hosts::{self, Reading};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Clouds start and stop on a host, so its free capacity is read again this often.
const FRESH: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(in crate::app) struct State {
    fetched: Option<Fetched<Vec<Reading>>>,
    job: Option<Job<Vec<Reading>>>,
    /// When the last reading failed, such as for settings not saved yet.
    failed_at: Option<Instant>,
}

impl State {
    /// Reads the hosts again when no reading is current. Saved settings restart it.
    pub(in crate::app::cloud_panel::production) fn request(&mut self, root: &Path, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach a host.
        if cfg!(test) {
            return;
        }
        let stale = self
            .fetched
            .as_ref()
            .is_none_or(|fetched| fetched.at.elapsed() >= FRESH);
        let paused = self.failed_at.is_some_and(|at| at.elapsed() < RETRY_FAILED);
        if self.job.is_none() && !paused && stale {
            let owned = root.to_owned();
            self.job = Some(spawn(root, ctx, move |settings, cancel| {
                Ok(hosts::readings(settings, &owned, cancel))
            }));
        }
        // A running read repaints when it ends; a current one wakes the dialog when it goes stale.
        if let Some(fetched) = self.fetched.as_ref().filter(|fetched| fetched.at.elapsed() < FRESH) {
            ctx.request_repaint_after(FRESH.saturating_sub(fetched.at.elapsed()));
        }
    }

    pub(in crate::app::cloud_panel::production) fn poll(&mut self) {
        match finished(&mut self.job) {
            Some(Ok(fetched)) => {
                self.fetched = Some(fetched);
                self.failed_at = None;
            }
            Some(Err(_)) => self.failed_at = Some(Instant::now()),
            None => {}
        }
    }

    /// Forgets readings taken with settings that were just replaced.
    pub(super) fn restart(&mut self) {
        *self = Self::default();
    }

    /// The saved hosts, as last read, while they are on.
    pub fn readings(&self) -> &[Reading] {
        self.fetched.as_ref().map_or(&[], |fetched| fetched.value.as_slice())
    }

    /// The last reading of `host` as it is saved now, if any.
    pub fn of(&self, host: &horizon_core::cloud_runtime::docker_host::Host) -> Option<&Reading> {
        self.readings().iter().find(|reading| reading.host == *host)
    }

    /// Keeps what a Check of a saved host found, until the next reading. A host that is
    /// not saved as checked is left out, so nothing offers it.
    pub fn record(&mut self, reading: Reading) {
        let saved = self
            .fetched
            .as_mut()
            .into_iter()
            .flat_map(|fetched| fetched.value.iter_mut());
        if let Some(kept) = saved.into_iter().find(|kept| kept.host == reading.host) {
            *kept = reading;
        }
    }

    /// The hosts' part of a `cloud_offers` answer, or `None` while they are first read and
    /// more than [`ANSWER_MARGIN_MILLIS`] remain until `deadline_in_millis`.
    pub fn sections(
        &self,
        requirements: &horizon_core::cloud_runtime::offers::Requirements,
        deadline_in_millis: i64,
    ) -> Option<Vec<serde_json::Value>> {
        // No host has a GPU or a region, so these requests wait for no reading.
        if requirements.gpu || requirements.region.is_some() {
            return Some(Vec::new());
        }
        let current = self.fetched.as_ref().filter(|fetched| fetched.at.elapsed() < FRESH);
        if current.is_none() && self.job.is_some() && deadline_in_millis > ANSWER_MARGIN_MILLIS {
            return None;
        }
        Some(hosts::section(self.readings(), requirements).into_iter().collect())
    }

    /// As if the hosts had just been read, for the dialog tests, which never reach one
    /// and run on Unix only.
    #[cfg(all(test, unix))]
    pub fn answered(&mut self, readings: Vec<Reading>) {
        self.fetched = Some(Fetched {
            value: readings,
            at: std::time::Instant::now(),
        });
    }
}
