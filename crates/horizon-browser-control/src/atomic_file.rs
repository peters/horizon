//! Atomic publication of private coordination files.
//!
//! Manifests, request queues, results and CLI job files are read by other
//! processes while they are rewritten. Every write lands in a sibling
//! temporary file that is flushed before it is moved into place, so a reader
//! observes either the previous or the next complete file.
//!
//! On Windows `std::fs::rename` falls back to a POSIX-semantics rename when
//! the destination is open, which succeeds for handles opened with
//! `FILE_SHARE_DELETE` (the `std::fs::File` default). A handle that denies
//! delete sharing, such as a scanner or an external reader, still blocks the
//! rename, so publication retries for a bounded window before failing.
//!
//! On Windows the replacing rename is not atomic for readers either. While it
//! is in flight, and a filter such as a scanner can hold it there for a few
//! hundred milliseconds, the destination can be missing or still being
//! deleted, so readers of replaced files open them through [`open`] or
//! [`read`].

use std::ffi::OsStr;
use std::fs::{DirEntry, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const BLOCKED_RETRY_WINDOW: Duration = Duration::from_secs(2);
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(1);
const MAX_RETRY_DELAY: Duration = Duration::from_millis(50);
const STAGED_PREFIX: &str = ".";
const STAGED_SUFFIX: &str = ".tmp";
/// Staging older than this was left by a writer that crashed mid-publication.
const ABANDONED_STAGING_AGE: Duration = Duration::from_secs(10);
/// Unix renames replace a destination atomically for readers; Windows ones
/// do not.
const RENAMES_TEAR_FOR_READERS: bool = cfg!(windows);

/// Replace `path` with `bytes`, creating it when it does not exist.
///
/// The new file is created `0600` on Unix and its data is synced before the
/// rename; the parent directory is flushed afterwards so the replacement
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

/// Open a file that [`replace`] publishes, for reading.
///
/// On Windows a destination that is missing or still being deleted while a
/// staged sibling shows a publication in flight is retried for the bounded
/// window; with no publication in flight a missing file is reported at once.
///
/// # Errors
/// Returns the open error, including `NotFound` for a file that does not
/// exist.
pub fn open(path: &Path) -> io::Result<File> {
    retry_while_blocked(
        || File::open(path),
        |error| is_racing_a_publication(error, path),
        BLOCKED_RETRY_WINDOW,
    )
    // The rename can complete, consuming its staging, between the failed open
    // and the staging check.
    .or_else(|error| {
        if is_torn_by_a_rename(&error) {
            File::open(path)
        } else {
            Err(error)
        }
    })
}

/// Read a file that [`replace`] publishes; see [`open`].
///
/// # Errors
/// Returns the error from opening or reading the file.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// List a directory whose files [`replace`] publishes, leaving staging out.
///
/// On Windows a listing taken while a replacing rename is in flight can miss
/// the destination, so a listing that shows fresh staging is retaken within
/// the bounded window; once the window elapses the last listing is returned.
///
/// # Errors
/// Returns the error from opening the directory.
pub fn read_dir(directory: &Path) -> io::Result<Vec<DirEntry>> {
    let entries = retake_while_torn(
        || Ok(std::fs::read_dir(directory)?.flatten().collect::<Vec<_>>()),
        |entries| RENAMES_TEAR_FOR_READERS && entries.iter().any(is_fresh_staging),
        BLOCKED_RETRY_WINDOW,
    )?;
    Ok(entries
        .into_iter()
        .filter(|entry| !is_staged_name(&entry.file_name()))
        .collect())
}

/// Create `path` with `bytes`, refusing to replace an existing file.
///
/// Publication moves the fully written temporary file into place without
/// replacement, so a reader never sees a partial file and a concurrent
/// creator cannot be overwritten. No reader can hold a destination that does
/// not exist yet, so this keeps the `atomicwrites` no-replace move
/// (`renameat2` or a hard link on Unix, `MoveFileExW` without replacement on
/// Windows), which also works on Windows volumes without hard links.
///
/// # Errors
/// Returns `AlreadyExists` when `path` exists, leaving that file untouched,
/// or the I/O error from staging, syncing or publishing the file.
pub fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = StagedFile::write(path, bytes)?;
    retry_while_blocked(
        || atomicwrites::move_atomic(&staged.path, path),
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
        if destination.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} has no file name", destination.display()),
            ));
        }
        let directory = parent_directory(destination);
        // A fixed-length leaf keeps staging valid for every destination the
        // filesystem accepts, however long its own leaf is.
        let path = directory.join(format!(
            "{STAGED_PREFIX}{}{STAGED_SUFFIX}",
            uuid::Uuid::new_v4().simple()
        ));
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

fn parent_directory(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn is_staged_name(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(STAGED_PREFIX))
        .and_then(|name| name.strip_suffix(STAGED_SUFFIX))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Fresh staging means a publication into its directory is in flight.
fn is_fresh_staging(entry: &DirEntry) -> bool {
    is_staged_name(&entry.file_name())
        && entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .map_or(true, |age| age < ABANDONED_STAGING_AGE)
            })
}

fn publication_in_flight(path: &Path) -> bool {
    std::fs::read_dir(parent_directory(path))
        .is_ok_and(|entries| entries.flatten().any(|entry| is_fresh_staging(&entry)))
}

fn is_racing_a_publication(error: &io::Error, path: &Path) -> bool {
    is_torn_by_a_rename(error) && publication_in_flight(path)
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

/// Flush the directory entry so a published file survives a power loss; on
/// Windows this replaces the write-through move `atomicwrites` used.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory)?
            .sync_all()?;
    }
    #[cfg(not(any(unix, windows)))]
    let _ = directory;
    Ok(())
}

fn retry_while_blocked<T>(
    mut attempt: impl FnMut() -> io::Result<T>,
    is_blocked: impl Fn(&io::Error) -> bool,
    window: Duration,
) -> io::Result<T> {
    let deadline = Instant::now() + window;
    let mut delay = FIRST_RETRY_DELAY;
    loop {
        match attempt() {
            Err(error) if is_blocked(&error) && Instant::now() < deadline => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(MAX_RETRY_DELAY);
            }
            result => return result,
        }
    }
}

fn retake_while_torn<T>(
    mut take: impl FnMut() -> io::Result<T>,
    is_torn: impl Fn(&T) -> bool,
    window: Duration,
) -> io::Result<T> {
    let deadline = Instant::now() + window;
    let mut delay = FIRST_RETRY_DELAY;
    loop {
        let taken = take()?;
        if !is_torn(&taken) || Instant::now() >= deadline {
            return Ok(taken);
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(MAX_RETRY_DELAY);
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

/// A Windows reader that opens the destination mid-rename finds it missing,
/// or still being deleted, which is reported as access denied.
fn is_torn_by_a_rename(error: &io::Error) -> bool {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    RENAMES_TEAR_FOR_READERS && matches!(error.raw_os_error(), Some(ERROR_FILE_NOT_FOUND | ERROR_ACCESS_DENIED))
}

#[cfg(test)]
mod tests;
