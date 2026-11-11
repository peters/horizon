//! The named secret values that this process gives to the agent panels it starts.
//!
//! A panel reads its environment once, when its program starts, so the values of
//! the named secrets must be in this process before a panel starts. A worker
//! thread reads them from the secrets file and publishes them here, and
//! [`apply`] copies them into the environment of one agent panel. Nothing else
//! reads this map, so no frame of the UI and no command line ever holds a value.
//!
//! The map is a process-global place on purpose: the environment of a panel is
//! inherited from this process, and the code that starts a panel is reached from
//! the board, from a preset, from a restored session and from an MCP operation
//! alike. One published snapshot is what all of those paths read.
//!
//! The read of a start answers after the first frames of that start, because it
//! waits on the disk. A panel that starts before the answer would keep the empty
//! map of the process, so [`expect_a_publish`] announces the read and an agent
//! panel waits for its answer with [`wait_until_published`], on the worker thread
//! of its start and never on a frame.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Condvar, Mutex, PoisonError, RwLock};
use std::time::Duration;

use zeroize::Zeroizing;

use super::SecretName;

static PUBLISHED: RwLock<BTreeMap<SecretName, Zeroizing<String>>> = RwLock::new(BTreeMap::new());

/// Whether a worker of this start announced a read of the store, and whether that
/// read published its answer. Both flags are one mutex, so the wait below cannot
/// miss an answer that arrives between the test and the wait.
#[derive(Clone, Copy, Default)]
struct Snapshot {
    due: bool,
    published: bool,
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot {
    due: false,
    published: false,
});
static SNAPSHOT_ANSWERED: Condvar = Condvar::new();

fn snapshot() -> std::sync::MutexGuard<'static, Snapshot> {
    SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says that a worker of this start reads the named secrets of this computer, so
/// an agent panel that starts before that answer waits for it.
///
/// The call takes no lock that a worker holds and reads no file, so the frame of
/// a start makes it before it starts the read.
pub fn expect_a_publish() {
    let mut snapshot = snapshot();
    snapshot.due = true;
    snapshot.published = false;
}

/// Replaces the values that the next agent panel inherits. It runs on the worker
/// thread that read the store, never on the UI thread.
pub fn publish(values: BTreeMap<SecretName, Zeroizing<String>>) {
    *PUBLISHED.write().unwrap_or_else(PoisonError::into_inner) = values;
    let mut answered = snapshot();
    if answered.due {
        answered.published = true;
    }
    drop(answered);
    SNAPSHOT_ANSWERED.notify_all();
}

/// Waits until the read that [`expect_a_publish`] announced published its answer,
/// or until `bound` ends. Answers `true` when the values of this process stand:
/// the read answered, or this start announced no read at all.
///
/// A panel must start even when the store does not answer, so the wait has a
/// bound. This runs on the worker thread of a panel start, never on the UI thread.
#[must_use]
pub fn wait_until_published(bound: Duration) -> bool {
    let snapshot = snapshot();
    if !snapshot.due || snapshot.published {
        return true;
    }
    let waited = SNAPSHOT_ANSWERED
        .wait_timeout_while(snapshot, bound, |snapshot| snapshot.due && !snapshot.published)
        .unwrap_or_else(PoisonError::into_inner);
    let snapshot = waited.0;
    !snapshot.due || snapshot.published
}

/// Drops every published value and the announcement of a read, for a test that
/// starts from no secret.
#[cfg(test)]
pub(crate) fn clear() {
    publish(BTreeMap::new());
    let mut snapshot = snapshot();
    snapshot.due = false;
    snapshot.published = false;
}

/// One test at a time reads and writes the published values: the map belongs to
/// the process, and two tests that publish at the same time would read each
/// other's secret. The guard drops the announcement of the read too, so a test
/// that leaves a read unanswered cannot make the next test wait for it.
#[cfg(test)]
pub(crate) fn exclusive() -> Exclusive {
    static TESTS: Mutex<()> = Mutex::new(());
    let guard = TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    clear();
    Exclusive { _tests: guard }
}

#[cfg(test)]
pub(crate) struct Exclusive {
    _tests: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for Exclusive {
    fn drop(&mut self) {
        clear();
    }
}

/// Publishes one value, as the store answers it after a write.
pub(crate) fn set(name: SecretName, value: Zeroizing<String>) {
    PUBLISHED
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name, value);
}

/// Drops one value, because its secret was deleted or its store lost it.
pub(crate) fn remove(name: &SecretName) {
    PUBLISHED.write().unwrap_or_else(PoisonError::into_inner).remove(name);
}

/// The names that this process holds a value for now.
#[must_use]
pub fn published_names() -> Vec<SecretName> {
    PUBLISHED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .keys()
        .cloned()
        .collect()
}

/// Copies every published value into the environment of one agent panel.
pub fn apply<S: std::hash::BuildHasher>(environment: &mut HashMap<String, String, S>) {
    for (name, value) in PUBLISHED.read().unwrap_or_else(PoisonError::into_inner).iter() {
        environment.insert(name.as_str().to_owned(), value.to_string());
    }
}
