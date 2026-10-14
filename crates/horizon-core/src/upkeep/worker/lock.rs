//! The lock of one Upkeep's saved choice.
use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex, PoisonError};

static HELD: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static RELEASED: Condvar = Condvar::new();

/// Holds the saved choice of one Upkeep until it is dropped.
#[must_use = "the lock ends when it is dropped"]
pub struct SettingsLock(String);

impl Drop for SettingsLock {
    fn drop(&mut self) {
        HELD.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.0);
        RELEASED.notify_all();
    }
}

/// Held while the saved choice of `cloud_id` is read, changed and saved again, so two
/// changes of one Upkeep from this Horizon (the board and an agent tool) never undo each
/// other, and while an exchange carries it to the worker, so a worker never gets a choice
/// older than the saved one. Another Upkeep's lock does not wait for it. Blocks: take it
/// off the UI thread.
pub fn settings_lock(cloud_id: &str) -> SettingsLock {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    while held.contains(cloud_id) {
        held = RELEASED.wait(held).unwrap_or_else(PoisonError::into_inner);
    }
    held.insert(cloud_id.to_owned());
    SettingsLock(cloud_id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_upkeep_waits_for_its_own_lock_only() {
        let first = settings_lock("lock-test-a");
        drop(settings_lock("lock-test-b"));
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _again = settings_lock("lock-test-a");
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
            "still held"
        );
        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
    }
}
