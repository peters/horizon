//! The sessions new cloud panels add to their cloud's record. A worker thread saves the
//! record, so a slow disk never stalls a frame. While that save runs it holds the record's
//! lock, so the record it saves is where the next panel looks and adds its session. A save
//! that fails is retried until it holds every session, and the cloud stays busy until then:
//! no operation saves a record that lacks the session of a running panel.
//!
//! A save belongs to its cloud, not to the runtime that started it: a session switch drops
//! the runtimes but keeps the clouds, so a later runtime of the same cloud finds the save
//! and joins or waits for it. A save ends only once it recorded every session; deleting
//! the cloud locks the record, so it cannot start while a save runs.
use super::{HorizonApp, Store, cloud_runtime};
use cloud_runtime::state::Deployment;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

/// How long a failed save waits before it tries again; each failure doubles it up to
/// [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_millis(250);
const LAST_RETRY: Duration = Duration::from_secs(5);

/// Every running save, by the cloud directory whose record it holds.
static FLIGHTS: Mutex<BTreeMap<PathBuf, Arc<Mutex<Flight>>>> = Mutex::new(BTreeMap::new());

#[derive(Default)]
pub(super) struct Sessions {
    /// The save this runtime started or joined.
    flight: RefCell<Option<Arc<Mutex<Flight>>>>,
    /// This runtime showed that save's failure.
    shown: Cell<bool>,
}

/// A record being saved, and how often it changed since the save began.
struct Flight {
    record: Deployment,
    generation: u64,
    /// The save ended and the record's lock is released.
    done: bool,
    /// Why the save failed, while it tries again.
    failure: Option<String>,
}

/// The save that runs for the cloud in `directory`, if any.
fn running(directory: &Path) -> Option<Arc<Mutex<Flight>>> {
    FLIGHTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(directory)
        .cloned()
}

impl Sessions {
    /// The sessions of a runtime that a session switch rebuilt; it joins the save that
    /// still runs for its cloud, so the cloud stays busy until that save ends.
    pub(super) fn of(directory: &Path) -> Self {
        let sessions = Self::default();
        if let Some(flight) = running(directory) {
            sessions.track(flight);
        }
        sessions
    }

    fn track(&self, flight: Arc<Mutex<Flight>>) {
        let tracked = self
            .flight
            .borrow()
            .as_ref()
            .is_some_and(|own| Arc::ptr_eq(own, &flight));
        if !tracked {
            *self.flight.borrow_mut() = Some(flight);
            self.shown.set(false);
        }
    }

    /// Runs `change` on the cloud's record in `directory`: the record a running save holds,
    /// or else the saved one, read under the record's lock without waiting for it. When
    /// `change` says it changed the record, a worker thread saves it.
    pub(super) fn with_record<T>(
        &self,
        directory: &Path,
        repaint: Option<egui::Context>,
        change: impl FnOnce(&mut Deployment) -> (T, bool),
    ) -> cloud_runtime::Result<T> {
        if let Some(flight) = running(directory) {
            self.track(Arc::clone(&flight));
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
                failure: None,
            }));
            self.track(Arc::clone(&flight));
            FLIGHTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(directory.to_path_buf(), Arc::clone(&flight));
            let directory = directory.to_path_buf();
            std::thread::spawn(move || save(store, &directory, &flight, repaint));
        }
        Ok(value)
    }

    /// Whether a save holds the record's lock, including one that failed and waits to
    /// try again. Operations that lock the record wait for it: the UI counts the cloud as
    /// busy, and work already started waits on its own thread through [`fence`].
    pub(super) fn saving(&self) -> bool {
        self.flight
            .borrow()
            .as_ref()
            .is_some_and(|flight| !flight.lock().unwrap_or_else(PoisonError::into_inner).done)
    }

    /// Why the save still fails, once for each runtime that holds it.
    pub(super) fn failure(&self) -> Option<String> {
        if self.shown.get() {
            return None;
        }
        let flight = self.flight.borrow();
        let flight = flight.as_ref()?.lock().unwrap_or_else(PoisonError::into_inner);
        let failure = flight.failure.clone().filter(|_| !flight.done)?;
        self.shown.set(true);
        Some(failure)
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
    pub(super) fn saving_for_test(directory: &Path, record: Deployment) -> Self {
        let flight = Arc::new(Mutex::new(Flight {
            record,
            generation: 0,
            done: false,
            failure: None,
        }));
        FLIGHTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(directory.to_path_buf(), Arc::clone(&flight));
        Self::of(directory)
    }

    /// The record a running save holds, which is newer than the saved one.
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

/// What a worker thread waits on before it locks the record of the cloud in `directory`:
/// the save that runs for that cloud, whichever runtime started it.
pub(super) fn fence(directory: &Path) -> Fence {
    Fence(running(directory))
}

/// The save running when it was taken, if any.
pub(super) struct Fence(Option<Arc<Mutex<Flight>>>);

impl Fence {
    /// Waits on this worker thread until that save recorded every session and released
    /// the record's lock.
    pub(super) fn wait(&self) {
        let Some(flight) = &self.0 else { return };
        while !flight.lock().unwrap_or_else(PoisonError::into_inner).done {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Saves the record until a save holds every change, then releases its lock. The lock is
/// released before the flight says it is done, so a reader that sees it done can lock. A
/// failed save is shown and tried again until it succeeds, unless the cloud's folder is
/// gone: no session can be recorded there, and what reads the record next finds it missing.
fn save(store: Store, directory: &Path, flight: &Arc<Mutex<Flight>>, repaint: Option<egui::Context>) {
    let mut retry = FIRST_RETRY;
    loop {
        let (record, generation) = {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            (flight.record.clone(), flight.generation)
        };
        match store.save(&record) {
            Ok(()) => {
                let held = flight.lock().unwrap_or_else(PoisonError::into_inner);
                if held.generation == generation {
                    land(store, held, directory, flight, repaint);
                    return;
                }
            }
            Err(error) if !directory.exists() => {
                tracing::warn!(directory = %directory.display(), %error, "a cloud's folder is gone; its sessions are not recorded");
                let held = flight.lock().unwrap_or_else(PoisonError::into_inner);
                land(store, held, directory, flight, repaint);
                return;
            }
            Err(error) => {
                let mut held = flight.lock().unwrap_or_else(PoisonError::into_inner);
                if held.failure.is_none() {
                    held.failure = Some(format!(
                        "Could not record the session of a new cloud panel: {error}. Horizon tries again; \
                         the cloud waits until it is recorded"
                    ));
                    if let Some(ctx) = &repaint {
                        ctx.request_repaint();
                    }
                }
                drop(held);
                std::thread::sleep(retry);
                retry = (retry * 2).min(LAST_RETRY);
            }
        }
    }
}

/// Ends a save: releases the record's lock, then says the save is done and forgets it.
/// `held` is the flight, locked since the save was found to hold every change, so no
/// panel adds a session between that check and the end.
fn land(
    store: Store,
    mut held: std::sync::MutexGuard<'_, Flight>,
    directory: &Path,
    flight: &Arc<Mutex<Flight>>,
    repaint: Option<egui::Context>,
) {
    drop(store);
    held.done = true;
    drop(held);
    let mut flights = FLIGHTS.lock().unwrap_or_else(PoisonError::into_inner);
    if flights
        .get(directory)
        .is_some_and(|running| Arc::ptr_eq(running, flight))
    {
        flights.remove(directory);
    }
    drop(flights);
    // The cloud is no longer busy.
    if let Some(ctx) = repaint {
        ctx.request_repaint();
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

        let fence = fence(&directory);
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
    fn a_save_outlives_a_session_switch_and_a_later_runtime_waits_for_it() {
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
        // A session switch drops the runtime, and with it its sessions; the cloud stays.
        drop(sessions);
        std::thread::sleep(FIRST_RETRY * 2);
        let rebuilt = Sessions::of(&directory);
        assert!(rebuilt.saving(), "the cloud of the new runtime is still busy");
        assert!(
            Store::lock(&directory).is_err(),
            "the save still holds the record's lock"
        );
        // A preparation of the new runtime waits for the save on its own thread.
        let fence = fence(&directory);
        let (prepared, ready) = std::sync::mpsc::channel();
        let preparation = std::thread::spawn(move || {
            fence.wait();
            prepared.send(()).unwrap();
        });
        assert!(
            ready.recv_timeout(FIRST_RETRY).is_err(),
            "the preparation waits while the save fails"
        );
        rebuilt
            .with_record(&directory, None, |record| {
                record.sessions.push(session("second"));
                ((), true)
            })
            .unwrap();
        set_writable(&directory, true);
        preparation.join().unwrap();
        assert!(!rebuilt.saving());
        assert_eq!(
            recorded(&directory),
            ["first", "second"],
            "the save records the session it held before the switch and the one added after"
        );
    }

    #[test]
    fn a_save_whose_cloud_folder_is_gone_ends() {
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
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while sessions.failure().is_none() {
            assert!(std::time::Instant::now() < deadline, "the save fails");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Something outside Horizon removes the folder while the save waits to retry.
        set_writable(&directory, true);
        std::fs::remove_dir_all(&directory).unwrap();
        fence(&directory).wait();
        assert!(!sessions.saving(), "the cloud is no longer busy");
        assert!(!directory.exists(), "the save does not make the folder again");
    }
}
