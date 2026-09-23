//! Atomic publication of private coordination files.
//!
//! Manifests, request queues, results and CLI job files are read by other
//! processes while they are rewritten. Every write lands in a sibling
//! temporary file that is flushed before it is renamed or linked into place,
//! so a reader observes either the previous or the next complete file.
//!
//! On Windows `std::fs::rename` falls back to a POSIX-semantics rename when
//! the destination is open, which succeeds for handles opened with
//! `FILE_SHARE_DELETE` (the `std::fs::File` default). A handle that denies
//! delete sharing, such as a scanner or an external reader, still blocks the
//! rename, so publication retries for a bounded window before failing.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BLOCKED_RETRY_WINDOW: Duration = Duration::from_secs(2);
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(1);
const MAX_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Replace `path` with `bytes`, creating it when it does not exist.
///
/// The new file is created `0600` on Unix and its data is synced before the
/// rename; the parent directory is synced afterwards so the replacement
/// survives a crash.
///
/// # Errors
/// Returns the I/O error from staging, syncing or publishing the file. On
/// Windows a destination held open without delete sharing is reported only
/// after the bounded retry window elapses.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = StagedFile::write(path, bytes)?;
    retry_while_blocked(
        || std::fs::rename(&staged.path, path),
        is_blocked_by_open_handle,
        BLOCKED_RETRY_WINDOW,
    )?;
    staged.published()
}

/// Create `path` with `bytes`, refusing to replace an existing file.
///
/// Publication links the fully written temporary file into place, so a
/// reader never sees a partial file and a concurrent creator cannot be
/// overwritten.
///
/// # Errors
/// Returns `AlreadyExists` when `path` exists, leaving that file untouched,
/// or the I/O error from staging, syncing or publishing the file.
pub fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = StagedFile::write(path, bytes)?;
    retry_while_blocked(
        || std::fs::hard_link(&staged.path, path),
        is_blocked_by_open_handle,
        BLOCKED_RETRY_WINDOW,
    )?;
    staged.published()
}

/// A flushed temporary sibling of the destination, removed unless published.
struct StagedFile {
    path: PathBuf,
    directory: PathBuf,
}

impl StagedFile {
    fn write(destination: &Path, bytes: &[u8]) -> io::Result<Self> {
        let file_name = destination.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} has no file name", destination.display()),
            )
        })?;
        let directory = match destination.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut staged_name = OsString::from(".");
        staged_name.push(file_name);
        staged_name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
        let path = directory.join(staged_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path)?;
        let staged = Self { path, directory };
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(staged)
    }

    /// Finish once the destination names the staged data: drop the staging
    /// name (already gone after a rename) and make the new entry durable.
    fn published(self) -> io::Result<()> {
        remove_staged(&self.path);
        sync_directory(&self.directory)
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        remove_staged(&self.path);
    }
}

fn remove_staged(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not remove a staged coordination file");
        }
    }
}

// NTFS journals the rename itself, and syncing a directory on Windows needs a
// write handle, so there the staged file is synced before publication only.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

fn retry_while_blocked(
    mut publish: impl FnMut() -> io::Result<()>,
    is_blocked: impl Fn(&io::Error) -> bool,
    window: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + window;
    let mut delay = FIRST_RETRY_DELAY;
    loop {
        match publish() {
            Err(error) if is_blocked(&error) && Instant::now() < deadline => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(MAX_RETRY_DELAY);
            }
            result => return result,
        }
    }
}

/// Windows reports a destination or staged file held open without delete
/// sharing as access denied, or as a sharing or lock violation.
#[cfg(windows)]
fn is_blocked_by_open_handle(error: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(
        error.raw_os_error(),
        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

/// Unix renames never wait for readers, so a permission error is genuine.
#[cfg(not(windows))]
fn is_blocked_by_open_handle(_error: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests;
