//! The session index file, which every Horizon process on one Horizon home changes.
//!
//! Only the session writer thread changes the index (see `writer.rs`), so the UI thread
//! never waits for its locks or a sync. A change reads, changes and writes the index while
//! it holds [`INDEX_UPDATE`] and a file lock on `index.lock` next to the index, so that no
//! thread or process writes over the change of another. The new index is written to a
//! uniquely named temporary file, synced, renamed into place, and its folder is synced.
use std::fs;
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::model::SessionIndex;
use super::{SESSION_INDEX_VERSION, writer};
use crate::error::{Error, Result};

/// Some file systems, such as NFS, lock a file for a whole process, so the threads of one
/// process also take this before the file lock.
static INDEX_UPDATE: Mutex<()> = Mutex::new(());

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
}

impl<'a> IndexFile<'a> {
    pub(super) fn at(path: &'a Path) -> Self {
        Self {
            path,
            threads: &INDEX_UPDATE,
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
        // Unlike `persist`, this rename also replaces an index that another process reads on
        // Windows.
        fs::rename(&renamed, self.path)?;
        renamed.disable_cleanup(true);
        writer::sync_folder(self.folder())?;
        // A Horizon from before this lock can still rename its index over this one, so the
        // file read must be the one written, and the path must still name it after the read.
        let mut named = fs::File::open(self.path)?;
        let mut contents = String::new();
        named.read_to_string(&mut contents)?;
        if !writer::same_file(&written, &named.metadata()?) || !writer::same_file(&written, &fs::metadata(self.path)?) {
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
    /// returned file is dropped.
    fn lock(&self) -> Result<fs::File> {
        fs::create_dir_all(self.folder())?;
        let lock_path = self.path.with_extension("lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(Error::State(format!(
                        "another Horizon process holds {}",
                        lock_path.display()
                    )));
                }
                Err(fs::TryLockError::Error(error)) => return Err(error.into()),
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
