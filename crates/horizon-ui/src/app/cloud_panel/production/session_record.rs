//! The sessions new cloud panels add to their cloud's record. A worker thread saves the
//! record, so a slow disk never stalls a frame. While that save runs it holds the record's
//! lock, so the record it saves is where the next panel looks and adds its session. A save
//! that fails is retried, and the cloud stays busy: no operation saves a record that lacks
//! the session of a running panel.
//!
//! A save that fails for [`RELEASE_AFTER`] releases the cloud, so the person can stop a
//! worker that bills: it gives up the record's lock, the cloud is no longer busy, and the
//! card warns that a panel's session is not in the record. Other operations, deletion
//! among them, then save the record, and each later try adds the sessions the record
//! lacks to the record it finds. A deleted cloud ends the save.
//!
//! A save belongs to its cloud, not to the runtime that started it: a session switch drops
//! the runtimes but keeps the clouds, so a later runtime of the same cloud finds the save
//! and joins or waits for it. A save ends once it recorded every session, or once its
//! cloud's folder or record is gone.
use super::{HorizonApp, Store, cloud_runtime};
use cloud_runtime::state::Deployment;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long a failed save waits before it tries again; each failure doubles it up to
/// [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_millis(250);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// How long a save may fail before it gives up the record's lock: then the cloud is no
/// longer busy, so the person can stop a worker that bills, and the save goes on trying.
#[cfg(not(test))]
const RELEASE_AFTER: Duration = Duration::from_secs(60);
#[cfg(test)]
const RELEASE_AFTER: Duration = Duration::from_secs(2);

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
    /// The save failed for [`RELEASE_AFTER`] and gave up the record's lock: the cloud is not
    /// busy, other operations save the record, and each later try adds the missing
    /// sessions to the record it then finds.
    released: bool,
}

/// The save that runs for the cloud in `directory`, if any.
fn running(directory: &Path) -> Option<Arc<Mutex<Flight>>> {
    FLIGHTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(directory)
        .cloned()
}

/// Whether every save ended, so that quitting now loses no recorded session.
fn all_ended() -> bool {
    // Released before any flight is locked, so no lock is ever held while the registry's
    // is taken or the other way round.
    let flights: Vec<_> = FLIGHTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    flights
        .iter()
        .all(|flight| flight.lock().unwrap_or_else(PoisonError::into_inner).done)
}

impl HorizonApp {
    /// Whether the sessions of new cloud panels are all in their records. Shutdown waits
    /// for this, within its time limit, so a quit does not drop a save that still runs.
    pub(in crate::app) fn cloud_sessions_recorded() -> bool {
        all_ended()
    }
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
            if !flight.done && flight.released {
                // Other operations save the record since the save released it: change the
                // record they saved, with the sessions the save still adds.
                let store = Store::lock(directory)?;
                let mut record = store
                    .load()?
                    .ok_or(cloud_runtime::Error::Invalid("Missing cloud deployment"))?;
                drop(store);
                add_missing_sessions(&mut record, &flight.record);
                let (value, changed) = change(&mut record);
                flight.record = record;
                if changed {
                    flight.generation += 1;
                }
                return Ok(value);
            }
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
                released: false,
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
        self.flight.borrow().as_ref().is_some_and(|flight| {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            !flight.done && !flight.released
        })
    }

    /// A save that failed for so long that it released the cloud: a panel's session is
    /// not in the record yet, so Stop, Resume or a rebuild would not restart that panel.
    pub(super) fn unrecorded(&self) -> bool {
        self.flight.borrow().as_ref().is_some_and(|flight| {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            !flight.done && flight.released
        })
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
        while self.saving() || self.unrecorded() {
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
            released: false,
        }));
        FLIGHTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(directory.to_path_buf(), Arc::clone(&flight));
        Self::of(directory)
    }

    /// A save that failed so long that it released the cloud, as [`Self::unrecorded`] says.
    #[cfg(all(test, unix))]
    pub(super) fn released_for_test(directory: &Path, record: Deployment) -> Self {
        let sessions = Self::saving_for_test(directory, record);
        if let Some(flight) = sessions.flight.borrow().as_ref() {
            flight.lock().unwrap_or_else(PoisonError::into_inner).released = true;
        }
        sessions
    }

    /// Ends the save that [`Self::saving_for_test`] made, so it holds no later exit check.
    #[cfg(all(test, unix))]
    pub(super) fn end_for_test(directory: &Path) {
        let removed = FLIGHTS.lock().unwrap_or_else(PoisonError::into_inner).remove(directory);
        if let Some(flight) = removed {
            flight.lock().unwrap_or_else(PoisonError::into_inner).done = true;
        }
    }

    /// The record a running save holds, which is newer than the saved one.
    pub(super) fn saving_record(&self) -> Option<Deployment> {
        let flight = self.flight.borrow();
        let flight = flight.as_ref()?.lock().unwrap_or_else(PoisonError::into_inner);
        // A released save's record is not the saved one: other operations save that.
        (!flight.done && !flight.released).then(|| flight.record.clone())
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
    /// the record's lock, or gave the lock up after failing for too long.
    pub(super) fn wait(&self) {
        let Some(flight) = &self.0 else { return };
        while {
            let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
            !flight.done && !flight.released
        } {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Saves the record until a save holds every change, then releases its lock. The lock is
/// released before the flight says it is done, so a reader that sees it done can lock. A
/// failed save is shown and tried again until it succeeds, unless the cloud's folder is
/// gone: no session can be recorded there, and what reads the record next finds it missing.
/// A save that fails for [`RELEASE_AFTER`] gives up the lock (see [`Flight::released`]).
fn save(store: Store, directory: &Path, flight: &Arc<Mutex<Flight>>, repaint: Option<egui::Context>) {
    let mut store = Some(store);
    let mut retry = FIRST_RETRY;
    let mut failing = None;
    loop {
        let error = match store.take().map_or_else(
            || attempt_released(directory, flight),
            |held| attempt_held(held, flight),
        ) {
            Attempt::Gone(held) => {
                tracing::warn!(directory = %directory.display(), "a cloud's record is gone; its sessions are not recorded");
                let flight_held = flight.lock().unwrap_or_else(PoisonError::into_inner);
                land(held, flight_held, directory, flight, repaint);
                return;
            }
            Attempt::Saved { held, generation, keep } => {
                // Checked under the flight's lock that marks the save done, so no panel adds
                // a session between this check and the end.
                let flight_held = flight.lock().unwrap_or_else(PoisonError::into_inner);
                if flight_held.generation == generation {
                    land(held, flight_held, directory, flight, repaint);
                    return;
                }
                drop(flight_held);
                store = if keep { held } else { None };
                failing = None;
                continue;
            }
            Attempt::Failed(held, error) => {
                store = held;
                error
            }
        };
        if std::fs::metadata(directory).is_err_and(|gone| gone.kind() == std::io::ErrorKind::NotFound) {
            // Only a folder that is really gone ends the save; an unreadable one is retried.
            tracing::warn!(directory = %directory.display(), %error, "a cloud's folder is gone; its sessions are not recorded");
            let flight_held = flight.lock().unwrap_or_else(PoisonError::into_inner);
            land(store, flight_held, directory, flight, repaint);
            return;
        }
        let since = *failing.get_or_insert_with(Instant::now);
        let mut held = flight.lock().unwrap_or_else(PoisonError::into_inner);
        if held.failure.is_none() {
            held.failure = Some(format!(
                "Could not record the session of a new cloud panel: {error}. Horizon tries again, \
                 and the cloud waits for it for up to a minute"
            ));
            if let Some(ctx) = &repaint {
                ctx.request_repaint();
            }
        }
        if store.is_some() && since.elapsed() >= RELEASE_AFTER {
            // The cloud waited long enough: Stop and the other operations may lock the
            // record again, and this save adds its sessions to the record it finds later.
            store = None;
            held.released = true;
            tracing::warn!(directory = %directory.display(), "a cloud panel session is still not recorded; the cloud is released");
            if let Some(ctx) = &repaint {
                ctx.request_repaint();
            }
        }
        drop(held);
        std::thread::sleep(retry);
        retry = (retry * 2).min(LAST_RETRY);
    }
}

/// What one try to save the record did. `held` is the record's lock, while held.
enum Attempt {
    /// Saved the flight's record at `generation`; `keep` the lock for the next try if a
    /// panel added a session meanwhile.
    Saved {
        held: Option<Store>,
        generation: u64,
        keep: bool,
    },
    /// The record is gone, so no session can be recorded.
    Gone(Option<Store>),
    Failed(Option<Store>, String),
}

/// Saves the flight's record under the lock the save holds.
fn attempt_held(store: Store, flight: &Arc<Mutex<Flight>>) -> Attempt {
    let (record, generation) = {
        let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
        (flight.record.clone(), flight.generation)
    };
    match store.save(&record) {
        Ok(()) => Attempt::Saved {
            held: Some(store),
            generation,
            keep: true,
        },
        Err(error) => Attempt::Failed(Some(store), error.to_string()),
    }
}

/// After the save released the cloud: locks the record if nothing else holds it, adds the
/// flight's sessions that the record lacks to the record as other operations saved it,
/// and saves that. A cloud whose folder or record is gone ends the save, and its folder
/// is not made again.
fn attempt_released(directory: &Path, flight: &Arc<Mutex<Flight>>) -> Attempt {
    let store = match Store::lock_existing(directory) {
        Ok(store) => store,
        Err(_) if std::fs::metadata(directory).is_err_and(|gone| gone.kind() == std::io::ErrorKind::NotFound) => {
            return Attempt::Gone(None);
        }
        Err(error) => return Attempt::Failed(None, error.to_string()),
    };
    let mut record = match store.load() {
        Ok(Some(record)) => record,
        Ok(None) => return Attempt::Gone(Some(store)),
        Err(error) => return Attempt::Failed(None, error.to_string()),
    };
    let generation = {
        let flight = flight.lock().unwrap_or_else(PoisonError::into_inner);
        add_missing_sessions(&mut record, &flight.record);
        flight.generation
    };
    match store.save(&record) {
        Ok(()) => Attempt::Saved {
            held: Some(store),
            generation,
            keep: false,
        },
        Err(error) => Attempt::Failed(None, error.to_string()),
    }
}

/// Adds to `record` each session of `from` that it lacks.
fn add_missing_sessions(record: &mut Deployment, from: &Deployment) {
    for session in &from.sessions {
        if !record.sessions.iter().any(|saved| saved.panel_id == session.panel_id) {
            record.sessions.push(session.clone());
        }
    }
}

/// Ends a save: releases the record's lock, then says the save is done and forgets it.
/// `held` is the flight, locked since the save was found to hold every change, so no
/// panel adds a session between that check and the end.
fn land(
    store: Option<Store>,
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

    #[test]
    fn a_save_that_still_runs_holds_the_exit_until_it_records() {
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
                record.sessions.push(session("panel"));
                ((), true)
            })
            .unwrap();
        // Saves of other tests can run in this process too, so this one is checked by its
        // folder: exit waits for every save that has not ended.
        let held =
            running(&directory).is_some_and(|flight| !flight.lock().unwrap_or_else(PoisonError::into_inner).done);
        assert!(held && !all_ended(), "a save that keeps failing holds the exit");
        set_writable(&directory, true);
        fence(&directory).wait();
        assert!(
            running(&directory).is_none(),
            "the recorded save no longer holds the exit"
        );
        assert_eq!(recorded(&directory), ["panel"]);
    }

    #[test]
    fn a_save_whose_cloud_folder_cannot_be_read_keeps_trying() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("clouds");
        let directory = parent.join("fixture");
        let record: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"fixture","repository":"/synthetic","revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker"},"spec":null,"sessions":[]
        }))
        .unwrap();
        Store::lock(&directory).unwrap().save(&record).unwrap();
        let sessions = Sessions::default();
        // The first save fails and retries; then the folder still exists but cannot be read.
        set_writable(&directory, false);
        sessions
            .with_record(&directory, None, |record| {
                record.sessions.push(session("panel"));
                ((), true)
            })
            .unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::thread::sleep(FIRST_RETRY * 3);
        let still_trying = sessions.saving();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        set_writable(&directory, true);
        assert!(still_trying, "an unreadable folder is not a deleted cloud");
        fence(&directory).wait();
        assert_eq!(
            recorded(&directory),
            ["panel"],
            "the session is recorded once the folder can be read"
        );
    }

    #[test]
    fn a_save_that_keeps_failing_releases_the_cloud_and_keeps_what_others_saved() {
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
                record.sessions.push(session("panel"));
                ((), true)
            })
            .unwrap();
        assert!(sessions.saving(), "the cloud waits for the save at first");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sessions.unrecorded() {
            assert!(Instant::now() < deadline, "the save did not release the cloud");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!sessions.saving(), "a released cloud is not busy");
        assert!(
            Sessions::of(&directory).saving_record().is_none(),
            "a runtime a session switch rebuilds reads the record from disk, not the save's copy"
        );
        fence(&directory).wait();
        // Stop saves the record while the save waits to try again.
        set_writable(&directory, true);
        let store = Store::lock(&directory).expect("the save gave up the record's lock");
        let mut stopped = store.load().unwrap().unwrap();
        assert!(stopped.sessions.is_empty(), "the session was not recorded yet");
        stopped.stage = cloud_runtime::Stage::Stopped;
        store.save(&stopped).unwrap();
        drop(store);
        // A panel added now sees the record Stop saved, not the save's old copy.
        let seen = sessions
            .with_record(&directory, None, |record| {
                record.sessions.push(session("later"));
                (record.stage, true)
            })
            .unwrap();
        assert_eq!(seen, cloud_runtime::Stage::Stopped, "a new panel reads what Stop saved");
        let deadline = Instant::now() + Duration::from_secs(30);
        while sessions.unrecorded() {
            assert!(Instant::now() < deadline, "the session was never recorded");
            std::thread::sleep(Duration::from_millis(5));
        }
        let saved = Store::lock(&directory).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.stage, cloud_runtime::Stage::Stopped, "what Stop saved is kept");
        let ids: Vec<_> = saved.sessions.iter().map(|session| session.panel_id.as_str()).collect();
        assert_eq!(ids, ["panel", "later"], "both sessions are added to that record");
    }
}
