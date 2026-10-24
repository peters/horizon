//! The session index file, which every Horizon process on one Horizon home changes.
//!
//! Only the session writer thread changes the index (see `writer.rs`), so the UI thread
//! never waits for its locks or a sync. A change reads, changes and writes the index while
//! it holds [`INDEX_UPDATE`] and a file lock on `index.lock` next to the index, so that no
//! thread or process writes over the change of another. The new index is written to a
//! uniquely named temporary file, synced, renamed into place, and its folder is synced.
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::Path;
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use super::model::SessionIndex;
use super::{SESSION_INDEX_VERSION, writer};
use crate::error::{Error, Result};

/// Some file systems, such as NFS, lock a file for a whole process, so the threads of one
/// process also take this before the file lock.
static INDEX_UPDATE: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// The index changes this thread started. Only a change waits for the index locks, so
    /// a test proves that a call on its thread never waits for them.
    pub(super) static CHANGES_STARTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How long a change waits for the file lock before it fails. A holder reads, writes and
/// syncs only the small index, so only a stopped process or a stalled disk holds it longer.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// A change of the index of one profile, which the session writer queues.
#[derive(Clone, Debug)]
pub(super) enum IndexChange {
    /// Names the session first.
    Touch(String),
    Remove(String),
}

impl IndexChange {
    pub(super) fn apply(&self, index: &mut SessionIndex, profile_id: &str) {
        match self {
            Self::Touch(session_id) => index.touch_profile_session(profile_id, session_id),
            Self::Remove(session_id) => index.remove_profile_session(profile_id, session_id),
        }
    }
}

/// The index at one path.
pub(super) struct IndexFile<'a> {
    path: &'a Path,
    /// [`INDEX_UPDATE`]; a test gives each updater its own, as each process has its own.
    threads: &'a Mutex<()>,
    /// How long a change waits for the file lock; [`LOCK_WAIT`] outside tests.
    wait: Duration,
}

impl<'a> IndexFile<'a> {
    pub(super) fn at(path: &'a Path) -> Self {
        Self {
            path,
            threads: &INDEX_UPDATE,
            wait: LOCK_WAIT,
        }
    }

    /// Reads the index; an index that is not there is empty.
    pub(super) fn load(&self) -> Result<SessionIndex> {
        match fs::read_to_string(self.path) {
            Ok(contents) => parse(&contents),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(SessionIndex::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Reads, changes and durably writes the index under the locks, and returns the index
    /// on disk after the change, read back under the locks.
    pub(super) fn change(&self, change: impl FnOnce(&mut SessionIndex)) -> Result<SessionIndex> {
        #[cfg(test)]
        CHANGES_STARTED.with(|started| started.set(started.get() + 1));
        let _thread = self.threads.lock().unwrap_or_else(PoisonError::into_inner);
        let _lock = self.lock()?;
        let mut index = self.load()?;
        change(&mut index);
        let yaml = serde_yaml::to_string(&index).map_err(|error| Error::State(error.to_string()))?;
        let mut file = tempfile::NamedTempFile::new_in(self.folder())?;
        file.write_all(yaml.as_bytes())?;
        file.as_file().sync_all()?;
        let written = file.as_file().metadata()?;
        let mut renamed = file.into_temp_path();
        // Unlike `persist`, this rename also replaces an index that another process reads.
        fs::rename(&renamed, self.path)?;
        renamed.disable_cleanup(true);
        writer::sync_folder(self.folder())?;
        // A Horizon from before this lock can still rename its index over this one. The path
        // must give the content written and, after that read, still name the synced file; only
        // a Unix host compares the file, and only there a save is durable.
        let contents = fs::read_to_string(self.path)?;
        if contents != yaml || !writer::same_file(&written, &fs::metadata(self.path)?) {
            return Err(Error::State(format!(
                "another program replaced {} while Horizon wrote it",
                self.path.display()
            )));
        }
        parse(&contents)
    }

    fn folder(&self) -> &'a Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }

    /// Makes the folder of the index and takes its file lock, which is held until the
    /// returned file is dropped. A file system that refuses the lock, for example an NFS mount
    /// without a lock service, gives no lock; then only [`INDEX_UPDATE`] keeps out other
    /// changes, as before this lock.
    fn lock(&self) -> Result<Option<fs::File>> {
        fs::create_dir_all(self.folder())?;
        let lock_path = self.path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let deadline = Instant::now() + self.wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(file)),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(Error::State(format!(
                        "another Horizon process holds {}",
                        lock_path.display()
                    )));
                }
                Err(fs::TryLockError::Error(error)) => {
                    static WARNED: Once = Once::new();
                    WARNED.call_once(|| {
                        tracing::warn!(%error, "cannot lock the session index; another Horizon can overwrite it");
                    });
                    return Ok(None);
                }
            }
        }
    }
}

fn parse(contents: &str) -> Result<SessionIndex> {
    let mut index = serde_yaml::from_str::<SessionIndex>(contents).map_err(|error| Error::State(error.to_string()))?;
    index.version = SESSION_INDEX_VERSION;
    Ok(index)
}

#[cfg(test)]
mod tests;
