//! The sessions new cloud panels add to their cloud's record. A worker thread saves the
//! record, so a slow disk never stalls a frame. While that save runs it holds the record's
//! lock, so the record it saves is where the next panel looks and adds its session.
use super::{HorizonApp, Store, cloud_runtime};
use cloud_runtime::state::Deployment;
use std::{
    cell::RefCell,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{Receiver, Sender, channel},
    },
};

pub(super) struct Sessions {
    flight: RefCell<Option<Arc<Mutex<Flight>>>>,
    failed: Sender<String>,
    failures: Receiver<String>,
}

impl Default for Sessions {
    fn default() -> Self {
        let (failed, failures) = channel();
        Self {
            flight: RefCell::default(),
            failed,
            failures,
        }
    }
}

/// A record being saved, and how often it changed since the save began.
struct Flight {
    record: Deployment,
    generation: u64,
    /// The save ended and the record's lock is released.
    done: bool,
}

impl Sessions {
    /// Runs `change` on the cloud's record in `directory`: the record a running save holds,
    /// or else the saved one, read under the record's lock without waiting for it. When
    /// `change` says it changed the record, a worker thread saves it.
    pub(super) fn with_record<T>(
        &self,
        directory: &Path,
        repaint: Option<egui::Context>,
        change: impl FnOnce(&mut Deployment) -> (T, bool),
    ) -> cloud_runtime::Result<T> {
        if let Some(flight) = self.flight.borrow().as_ref() {
            let mut flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            if !flight.done {
                let (value, changed) = change(&mut flight.record);
                if changed {
                    flight.generation += 1;
                }
                return Ok(value);
            }
        }
        let store = Store::lock(directory)?;
        let mut record = store
            .load()?
            .ok_or(cloud_runtime::Error::Invalid("Missing cloud deployment"))?;
        let (value, changed) = change(&mut record);
        if changed {
            let flight = Arc::new(Mutex::new(Flight {
                record,
                generation: 0,
                done: false,
            }));
            *self.flight.borrow_mut() = Some(Arc::clone(&flight));
            let failed = self.failed.clone();
            std::thread::spawn(move || save(store, &flight, &failed, repaint));
        }
        Ok(value)
    }

    /// A save that failed since the last call.
    pub(super) fn failure(&self) -> Option<String> {
        self.failures.try_iter().last()
    }

    /// Waits until no save runs. Tests use it before they read the record themselves.
    #[cfg(test)]
    pub(super) fn wait(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while let Some(flight) = self.flight.borrow().as_ref()
            && !flight.lock().unwrap_or_else(PoisonError::into_inner).done
        {
            assert!(std::time::Instant::now() < deadline, "a session save did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A running save that holds `record`, as the save of a new panel's session does.
    #[cfg(test)]
    pub(super) fn saving(record: Deployment) -> Self {
        let sessions = Self::default();
        *sessions.flight.borrow_mut() = Some(Arc::new(Mutex::new(Flight {
            record,
            generation: 0,
            done: false,
        })));
        sessions
    }

    /// The record a running save holds.
    #[cfg(test)]
    pub(super) fn saving_record(&self) -> Option<Deployment> {
        let flight = self.flight.borrow();
        let flight = flight.as_ref()?.lock().unwrap_or_else(PoisonError::into_inner);
        (!flight.done).then(|| flight.record.clone())
    }
}

impl HorizonApp {
    /// Shows a session save that failed; the panel it was for already runs.
    pub(super) fn report_failed_session_saves(&mut self) {
        let failed = self
            .cloud_prototype
            .production
            .runtimes
            .values()
            .filter_map(|runtime| runtime.sessions.failure())
            .last();
        if let Some(error) = failed {
            self.cloud_prototype.error = Some(error);
        }
    }
}

/// Saves the record until a save holds every change, then releases its lock. The lock is
/// released before the flight says it is done, so a reader that sees it done can lock.
fn save(store: Store, flight: &Mutex<Flight>, failed: &Sender<String>, repaint: Option<egui::Context>) {
    loop {
        let (record, generation) = {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            (flight.record.clone(), flight.generation)
        };
        let saved = store.save(&record);
        let mut flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
        if saved.is_err() || flight.generation == generation {
            drop(store);
            flight.done = true;
            drop(flight);
            if let Err(error) = saved {
                let _ = failed.send(format!("Could not record the session of a new cloud panel: {error}"));
                if let Some(ctx) = repaint {
                    ctx.request_repaint();
                }
            }
            return;
        }
    }
}
