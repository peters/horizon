//! The lock of one Upkeep's saved choice: a guard for the threads of this Horizon, and a
//! file lock for other Horizon processes that share its home.
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, PoisonError};

static HELD: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static RELEASED: Condvar = Condvar::new();

/// Holds the saved choice of one Upkeep until it is dropped.
#[must_use = "the lock ends when it is dropped"]
pub struct SettingsLock {
    cloud: String,
    /// The locked file once taken; closing it releases the file lock.
    file: Option<File>,
}

impl Drop for SettingsLock {
    fn drop(&mut self) {
        // The file lock goes first, so this process never holds it without the guard.
        drop(self.file.take());
        HELD.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.cloud);
        RELEASED.notify_all();
    }
}

/// Held while the saved choice of `cloud_id` is read, changed and saved again, so two
/// changes of one Upkeep (the board and an agent tool, also in two Horizon windows) never
/// undo each other, and while an exchange carries it to the worker, so a worker never gets
/// a choice older than the saved one. Another Upkeep's lock does not wait for it. Blocks:
/// take it off the UI thread.
/// # Errors
/// A cloud id that is not one folder name, or a lock file that cannot be opened or locked.
pub fn settings_lock(cloud_id: &str) -> std::io::Result<SettingsLock> {
    if !super::valid_cloud_id(cloud_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid cloud for Upkeep",
        ));
    }
    let mut lock = {
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        while held.contains(cloud_id) {
            held = RELEASED.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        held.insert(cloud_id.to_owned());
        // From here an error gives the guard back when `lock` drops.
        SettingsLock {
            cloud: cloud_id.to_owned(),
            file: None,
        }
    };
    let path = lock_file(cloud_id);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    file.lock()?;
    lock.file = Some(file);
    Ok(lock)
}

/// The lock file of `cloud_id`, next to its saved choice.
#[cfg(not(test))]
fn lock_file(cloud_id: &str) -> PathBuf {
    super::directory(cloud_id).join("settings.lock")
}

/// A test keeps its lock files to itself, never in the person's Horizon home.
#[cfg(test)]
fn lock_file(cloud_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("horizon-test-upkeep-locks-{}", std::process::id()))
        .join(cloud_id)
        .join("settings.lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_upkeep_waits_for_its_own_lock_only() {
        let first = settings_lock("lock-test-a").unwrap();
        drop(settings_lock("lock-test-b").unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _again = settings_lock("lock-test-a").unwrap();
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

    #[test]
    fn another_horizon_waits_for_the_file_lock() {
        let held = settings_lock("lock-test-c").unwrap();
        // Another Horizon opens the file on its own: here, a second open file.
        let other = OpenOptions::new().write(true).open(lock_file("lock-test-c")).unwrap();
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(held);
        other.try_lock().unwrap();
    }

    #[test]
    fn a_cloud_that_is_not_one_folder_has_no_lock() {
        assert!(settings_lock("../c1").is_err());
        assert!(!HELD.lock().unwrap().contains("../c1"));
    }
}
