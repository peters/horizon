//! What each saved Docker host can take now, read in the background for New cloud,
//! Cloud settings and agents' offer requests. Nothing is reserved.
use super::{ANSWER_MARGIN_MILLIS, Fetched, Job, RETRY_FAILED, finished, spawn};
use horizon_core::cloud_runtime::docker_host::hosts::{self, Reading};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime},
};

/// Clouds start and stop on a host, so its free capacity is read again this often.
const FRESH: Duration = Duration::from_secs(30);
/// How often a worker thread looks whether the settings file changed, also by another
/// Horizon, so a removed or switched-off host stops being offered.
const SETTINGS_CHECK: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(in crate::app) struct State {
    fetched: Option<Fetched<Vec<Reading>>>,
    job: Option<Job<Vec<Reading>>>,
    /// When the last reading failed, such as for settings not saved yet.
    failed_at: Option<Instant>,
    /// When the running reading started; a reading is as old as its start.
    started: Option<Instant>,
    /// When the settings file was saved, as last looked at.
    settings_saved: Option<SystemTime>,
    saved_check: Option<Receiver<Option<SystemTime>>>,
    checked_at: Option<Instant>,
    /// Whether a look has answered, so the next answer can tell a change.
    looked: bool,
}

impl State {
    /// Reads the hosts again when no reading is current. Saved settings restart it.
    pub(in crate::app::cloud_panel::production) fn request(&mut self, root: &Path, ctx: &egui::Context) {
        // UI tests resolve the developer's real Horizon home; they must never reach a host.
        if cfg!(test) {
            return;
        }
        self.look_at_settings(root);
        let stale = self
            .fetched
            .as_ref()
            .is_none_or(|fetched| fetched.at.elapsed() >= FRESH);
        let paused = self.failed_at.is_some_and(|at| at.elapsed() < RETRY_FAILED);
        if self.job.is_none() && !paused && stale {
            let owned = root.to_owned();
            self.started = Some(Instant::now());
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
                self.fetched = Some(Fetched {
                    at: self.started.unwrap_or(fetched.at),
                    value: fetched.value,
                });
                self.failed_at = None;
            }
            Some(Err(_)) => self.failed_at = Some(Instant::now()),
            None => {}
        }
    }

    /// Forgets the readings once a worker thread finds that the settings file changed,
    /// and starts the next look every [`SETTINGS_CHECK`].
    fn look_at_settings(&mut self, root: &Path) {
        if let Some(receiver) = &self.saved_check {
            match receiver.try_recv() {
                Ok(saved) => {
                    self.saved_check = None;
                    self.settings_looked_at(saved);
                }
                Err(TryRecvError::Disconnected) => self.saved_check = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.saved_check.is_none() && self.checked_at.is_none_or(|at| at.elapsed() >= SETTINGS_CHECK) {
            let (sender, receiver) = mpsc::channel();
            let path = root.join("settings.json");
            std::thread::spawn(move || {
                let _ = sender.send(std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok());
            });
            self.saved_check = Some(receiver);
            self.checked_at = Some(Instant::now());
        }
    }

    /// The first look sets what later looks compare with.
    fn settings_looked_at(&mut self, saved: Option<SystemTime>) {
        if self.looked && saved != self.settings_saved {
            // A reading on its way read the settings before the change.
            self.job = None;
            self.fetched = None;
            self.failed_at = None;
        }
        self.settings_saved = saved;
        self.looked = true;
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

    /// Whether the first reading is still on its way, so no host is known yet.
    pub fn first_reading(&self) -> bool {
        self.fetched.is_none() && self.job.is_some()
    }

    /// Whether New cloud may offer a saved host: the last reading found one.
    pub fn bound(&self) -> bool {
        !self.readings().is_empty()
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
        if !hosts::wanted(requirements) {
            return Some(Vec::new());
        }
        // A reading older than FRESH, also one whose refresh failed, offers nothing: free
        // capacity changes as clouds start and stop.
        let Some(current) = self.fetched.as_ref().filter(|fetched| fetched.at.elapsed() < FRESH) else {
            if self.job.is_some() && deadline_in_millis > ANSWER_MARGIN_MILLIS {
                return None;
            }
            return Some(Vec::new());
        };
        Some(hosts::section(&current.value, requirements).into_iter().collect())
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::{
        docker_host::{Capacity, Host},
        offers::Requirements,
    };

    #[test]
    fn only_a_current_reading_is_offered() {
        let reading = Reading {
            host: serde_json::from_value::<Host>(serde_json::json!({"name": "build-box", "access": "this-pc"}))
                .unwrap(),
            capacity: Ok(Capacity {
                cpus: 8,
                memory_bytes: 16 << 30,
                free_nano_cpus: 8_000_000_000,
                free_memory_bytes: 16 << 30,
                architecture: "x86_64".into(),
                version: "28.5.1".into(),
            }),
        };
        let mut state = State::default();
        state.answered(vec![reading.clone()]);
        assert_eq!(state.sections(&Requirements::default(), 0).unwrap().len(), 1);
        state.fetched = Some(Fetched {
            value: vec![reading],
            at: Instant::now().checked_sub(FRESH * 2).unwrap(),
        });
        assert_eq!(state.sections(&Requirements::default(), 0), Some(Vec::new()));
    }

    #[test]
    fn settings_saved_elsewhere_forget_the_readings() {
        let reading = Reading {
            host: serde_json::from_value::<Host>(serde_json::json!({"name": "build-box", "access": "this-pc"}))
                .unwrap(),
            capacity: Err("not reachable".into()),
        };
        let first = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let mut state = State::default();
        state.answered(vec![reading]);
        // The first look only notes when the settings were saved.
        state.settings_looked_at(Some(first));
        assert_eq!(state.readings().len(), 1);
        state.settings_looked_at(Some(first));
        assert_eq!(state.readings().len(), 1);
        state.settings_looked_at(Some(first + Duration::from_secs(1)));
        assert!(state.readings().is_empty());
    }
}
