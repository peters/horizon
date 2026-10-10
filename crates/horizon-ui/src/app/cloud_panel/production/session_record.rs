//! The sessions new cloud panels add to their cloud's record. A worker thread saves the
//! record, so a slow disk never stalls a frame. While that save runs it holds the record's
//! lock, so the record it saves is where the next panel looks and adds its session. A save
//! that fails is retried until it holds every session, and the cloud stays busy until then:
//! no operation saves a record that lacks the session of a running panel.
use super::{HorizonApp, Store, cloud_runtime};
use cloud_runtime::state::Deployment;
use std::{
    cell::RefCell,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        mpsc::{Receiver, Sender, channel},
    },
    time::Duration,
};

/// How long a failed save waits before it tries again; each failure doubles it up to
/// [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_millis(250);
const LAST_RETRY: Duration = Duration::from_secs(5);

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

    /// Whether a save holds the record's lock, including one that failed and waits to
    /// try again. Operations that lock the record wait for it: the UI counts the cloud as
    /// busy, and work already started waits on its own thread through [`Self::fence`].
    pub(super) fn saving(&self) -> bool {
        self.flight
            .borrow()
            .as_ref()
            .is_some_and(|flight| !flight.lock().unwrap_or_else(PoisonError::into_inner).done)
    }

    /// What a worker thread waits on before it locks the record.
    pub(super) fn fence(&self) -> Fence {
        Fence(self.flight.borrow().as_ref().map(Arc::downgrade))
    }

    /// A save that failed since the last call.
    pub(super) fn failure(&self) -> Option<String> {
        self.failures.try_iter().last()
    }

    /// Waits until no save runs. Tests use it before they read the record themselves.
    #[cfg(all(test, unix))]
    pub(super) fn wait(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while self.saving() {
            assert!(std::time::Instant::now() < deadline, "a session save did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A running save that holds `record`, as the save of a new panel's session does.
    #[cfg(all(test, unix))]
    pub(super) fn saving_for_test(record: Deployment) -> Self {
        let sessions = Self::default();
        *sessions.flight.borrow_mut() = Some(Arc::new(Mutex::new(Flight {
            record,
            generation: 0,
            done: false,
        })));
        sessions
    }

    /// The record a running save holds.
    #[cfg(all(test, unix))]
    pub(super) fn saving_record(&self) -> Option<Deployment> {
        let flight = self.flight.borrow();
        let flight = flight.as_ref()?.lock().unwrap_or_else(PoisonError::into_inner);
        (!flight.done).then(|| flight.record.clone())
    }
}

impl HorizonApp {
    /// Shows a session save that failed; the panel it was for already runs, and the save
    /// tries again.
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

/// The save running when it was taken, if any.
pub(super) struct Fence(Option<Weak<Mutex<Flight>>>);

impl Fence {
    /// Waits on this worker thread until that save recorded every session and released
    /// the record's lock, or until its cloud is gone.
    pub(super) fn wait(&self) {
        let Some(flight) = &self.0 else { return };
        while let Some(flight) = flight.upgrade()
            && !flight.lock().unwrap_or_else(PoisonError::into_inner).done
        {
            drop(flight);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Saves the record until a save holds every change, then releases its lock. The lock is
/// released before the flight says it is done, so a reader that sees it done can lock. A
/// failed save is reported once and tried again, until its cloud's sessions are gone.
fn save(store: Store, flight: &Arc<Mutex<Flight>>, failed: &Sender<String>, repaint: Option<egui::Context>) {
    let mut retry = FIRST_RETRY;
    let mut reported = false;
    loop {
        let (record, generation) = {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            (flight.record.clone(), flight.generation)
        };
        match store.save(&record) {
            Ok(()) => {
                let mut flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
                if flight.generation == generation {
                    drop(store);
                    flight.done = true;
                    drop(flight);
                    // The cloud is no longer busy.
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                    return;
                }
            }
            Err(error) => {
                if !reported {
                    reported = true;
                    let _ = failed.send(format!(
                        "Could not record the session of a new cloud panel: {error}. Horizon tries again; \
                         the cloud waits until it is recorded"
                    ));
                    if let Some(ctx) = &repaint {
                        ctx.request_repaint();
                    }
                }
                std::thread::sleep(retry);
                retry = (retry * 2).min(LAST_RETRY);
                // A cloud that is gone has no session left to record.
                if Arc::strong_count(flight) == 1 {
                    return;
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn session(panel_id: &str) -> cloud_runtime::state::Session {
        cloud_runtime::state::Session {
            panel_id: panel_id.into(),
            agent: "shell".into(),
            tmux: panel_id.into(),
            branch: String::new(),
            worktree: "/workspace/checkout".into(),
        }
    }

    fn set_writable(directory: &Path, writable: bool) {
        let mode = if writable { 0o700 } else { 0o500 };
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn recorded(directory: &Path) -> Vec<String> {
        let saved = Store::lock(directory).unwrap().load().unwrap().unwrap();
        saved.sessions.into_iter().map(|session| session.panel_id).collect()
    }

    #[test]
    fn a_failed_save_keeps_the_cloud_busy_and_records_once_it_can() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fixture");
        let record: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"fixture","repository":"/synthetic","revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker"},"spec":null,"sessions":[]
        }))
        .unwrap();
        Store::lock(&directory).unwrap().save(&record).unwrap();
        let sessions = Sessions::default();
        // The record's folder takes no new file, so the save fails.
        set_writable(&directory, false);
        sessions
            .with_record(&directory, None, |record| {
                record.sessions.push(session("first"));
                ((), true)
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let failure = loop {
            if let Some(failure) = sessions.failure() {
                break failure;
            }
            assert!(std::time::Instant::now() < deadline, "the failure shows");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(failure.contains("tries again"), "{failure}");
        assert!(
            sessions.saving(),
            "no operation saves the record that lacks the session"
        );
        sessions
            .with_record(&directory, None, |record| {
                record.sessions.push(session("second"));
                ((), true)
            })
            .unwrap();

        let fence = sessions.fence();
        let waiter = std::thread::spawn(move || fence.wait());
        set_writable(&directory, true);
        sessions.wait();
        waiter.join().unwrap();
        assert_eq!(
            recorded(&directory),
            ["first", "second"],
            "the retry records every session the failed save held"
        );
        assert!(sessions.failure().is_none(), "a failure is reported once");
    }

    #[test]
    fn a_save_whose_cloud_is_gone_stops_trying() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fixture");
        let record: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"fixture","repository":"/synthetic","revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker"},"spec":null,"sessions":[]
        }))
        .unwrap();
        Store::lock(&directory).unwrap().save(&record).unwrap();
        let sessions = Sessions::default();
        set_writable(&directory, false);
        sessions
            .with_record(&directory, None, |record| {
                record.sessions.push(session("first"));
                ((), true)
            })
            .unwrap();
        let fence = sessions.fence();
        drop(sessions);
        // The waiter returns, and the save ends and releases the record's lock.
        fence.wait();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while Store::lock(&directory).is_err() {
            assert!(std::time::Instant::now() < deadline, "the save released the record");
            std::thread::sleep(Duration::from_millis(20));
        }
        set_writable(&directory, true);
    }
}
