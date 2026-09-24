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
use std::fs::{File, OpenOptions};
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

/// A freshly staged sibling means some publication into this directory is in
/// flight.
fn publication_in_flight(path: &Path) -> bool {
    std::fs::read_dir(parent_directory(path)).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            is_staged_name(&entry.file_name())
                && entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| {
                        SystemTime::now()
                            .duration_since(modified)
                            .map_or(true, |age| age < ABANDONED_STAGING_AGE)
                    })
        })
    })
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

/// A reader that opens the destination mid-rename finds it missing, or still
/// being deleted, which Windows reports as access denied.
#[cfg(windows)]
fn is_torn_by_a_rename(error: &io::Error) -> bool {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    matches!(error.raw_os_error(), Some(ERROR_FILE_NOT_FOUND | ERROR_ACCESS_DENIED))
}

/// Unix renames replace the destination atomically for readers.
#[cfg(not(windows))]
fn is_torn_by_a_rename(_error: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::{StagedFile, create_new, is_staged_name, publication_in_flight, read, replace, retry_while_blocked};

    fn entry_names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replace_creates_then_overwrites_without_leaving_staged_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");

        replace(&path, b"first").unwrap();
        replace(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(entry_names(root.path()), ["panel.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn replace_publishes_owner_only_files() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        replace(&path, b"{}").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "group and other bits must stay clear, got {mode:o}");
    }

    #[test]
    fn replace_reports_a_missing_directory_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing").join("panel.json");

        let error = replace(&path, b"{}").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(entry_names(root.path()).is_empty());
    }

    /// Before #847 the Windows replace used `MoveFileExW`, which fails with
    /// access denied whenever any handle has the destination open.
    #[test]
    fn replace_succeeds_while_a_reader_holds_the_destination_open() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        replace(&path, b"old").unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();

        replace(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let mut held = Vec::new();
        io::Read::read_to_end(&mut reader, &mut held).unwrap();
        assert_eq!(held, b"old", "an open reader keeps the file it opened");
        drop(reader);
        assert_eq!(entry_names(root.path()), ["panel.json"]);
    }

    #[test]
    fn concurrent_readers_never_fail_a_replace_or_observe_a_partial_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let first = vec![b'a'; 64 * 1024];
        let second = vec![b'b'; 64 * 1024];
        replace(&path, &first).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let reader_count = 4;
        let polling = Arc::new(Barrier::new(reader_count + 1));
        let readers: Vec<_> = (0..reader_count)
            .map(|_| {
                let path = path.clone();
                let stop = Arc::clone(&stop);
                let polling = Arc::clone(&polling);
                let (first, second) = (first.clone(), second.clone());
                std::thread::spawn(move || {
                    let mut reads = 0_u32;
                    loop {
                        let contents = read(&path).unwrap();
                        assert!(contents == first || contents == second, "observed a partial file");
                        reads += 1;
                        if reads == 1 {
                            polling.wait();
                        }
                        if stop.load(Ordering::Relaxed) {
                            return reads;
                        }
                    }
                })
            })
            .collect();
        polling.wait();

        for round in 0..200 {
            let contents = if round % 2 == 0 { &second } else { &first };
            replace(&path, contents).unwrap();
        }
        stop.store(true, Ordering::Relaxed);

        for reader in readers {
            assert!(reader.join().unwrap() > 0);
        }
        assert_eq!(entry_names(root.path()), ["panel.json"]);
    }

    #[test]
    fn only_staging_names_mark_a_publication_in_flight() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        replace(&path, b"{}").unwrap();
        std::fs::write(root.path().join(".notes.tmp"), b"").unwrap();
        std::fs::write(root.path().join(format!(".{}.tmp", "g".repeat(32))), b"").unwrap();
        assert!(!publication_in_flight(&path));

        let staged = StagedFile::write(&path, b"next").unwrap();
        assert!(is_staged_name(staged.path.file_name().unwrap()));
        assert!(publication_in_flight(&path));
        drop(staged);
        assert!(!publication_in_flight(&path));
    }

    #[test]
    fn abandoned_staging_does_not_mark_a_publication_in_flight() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let orphan = std::fs::File::create(root.path().join(format!(".{}.tmp", "b".repeat(32)))).unwrap();
        orphan
            .set_modified(std::time::SystemTime::now() - super::ABANDONED_STAGING_AGE * 2)
            .unwrap();
        drop(orphan);

        assert!(!publication_in_flight(&path));
    }

    #[test]
    fn read_reports_an_absent_file_at_once() {
        let root = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();

        let error = read(&root.path().join("panel.json")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "no publication was in flight"
        );
    }

    /// Windows can leave the destination missing while a replacing rename is
    /// in flight; the staged sibling is still there, so the read waits.
    #[cfg(windows)]
    #[test]
    fn read_waits_out_a_publication_that_is_mid_rename() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let staged = root.path().join(format!(".{}.tmp", "a".repeat(32)));
        std::fs::write(&staged, b"new").unwrap();
        let publish = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                std::fs::rename(staged, path).unwrap();
            })
        };

        assert_eq!(read(&path).unwrap(), b"new");
        publish.join().unwrap();
    }

    /// Unix renames are atomic for readers, so a missing file is genuine even
    /// while a sibling is being published.
    #[cfg(not(windows))]
    #[test]
    fn read_does_not_wait_for_publications_on_unix() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        let _staged = StagedFile::write(&path, b"next").unwrap();
        let started = std::time::Instant::now();

        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn replace_accepts_a_destination_leaf_near_the_name_limit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(format!("{}.json", "p".repeat(245)));

        replace(&path, b"first").unwrap();
        replace(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(entry_names(root.path()).len(), 1);
    }

    #[test]
    fn create_new_refuses_an_existing_file_and_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("step-0.json");

        create_new(&path, b"first").unwrap();
        let error = create_new(&path, b"second").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(entry_names(root.path()), ["step-0.json"]);
    }

    #[test]
    fn blocked_publication_retries_until_it_succeeds() {
        let attempts = Cell::new(0);
        let result = retry_while_blocked(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 3 {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    Ok(())
                }
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            Duration::from_secs(5),
        );

        result.unwrap();
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn blocked_publication_gives_up_after_the_window() {
        let attempts = Cell::new(0);
        let error = retry_while_blocked::<()>(
            || {
                attempts.set(attempts.get() + 1);
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            Duration::from_millis(20),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(attempts.get() > 1, "a blocked publication is retried at least once");
    }

    #[test]
    fn unrelated_publication_errors_are_not_retried() {
        let attempts = Cell::new(0);
        let error = retry_while_blocked::<()>(
            || {
                attempts.set(attempts.get() + 1);
                Err(io::Error::from(io::ErrorKind::NotFound))
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            Duration::from_secs(5),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(attempts.get(), 1);
    }

    /// A reader that denies delete sharing blocks even a POSIX-semantics rename;
    /// the replace waits for it instead of failing.
    #[cfg(windows)]
    #[test]
    fn replace_waits_for_a_reader_that_denies_delete_sharing() {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        replace(&path, b"old").unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(reader);
        });

        replace(&path, b"new").unwrap();

        release.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(entry_names(root.path()), ["panel.json"]);
    }

    #[cfg(windows)]
    #[test]
    fn only_sharing_failures_count_as_blocked_on_windows() {
        use super::is_blocked_by_open_handle;

        assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(5)));
        assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(32)));
        assert!(is_blocked_by_open_handle(&io::Error::from_raw_os_error(33)));
        assert!(!is_blocked_by_open_handle(&io::Error::from_raw_os_error(2)));
        assert!(!is_blocked_by_open_handle(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
    }
}
