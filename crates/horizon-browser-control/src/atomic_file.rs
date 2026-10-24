//! Atomic publication of private coordination files.
//!
//! Manifests, request queues, results and CLI job files are read by other
//! processes while they are rewritten. Every write lands in a sibling
//! temporary file that is flushed before it is moved into place, so a reader
//! observes either the previous or the next complete file.

use std::ffi::OsStr;
use std::fs::{DirEntry, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const STAGED_PREFIX: &str = ".";
const STAGED_SUFFIX: &str = ".tmp";

/// Replace `path` with `bytes`, creating it when it does not exist.
///
/// The new file is created `0600` on Unix and its data is synced before the
/// rename; the parent directory is flushed afterwards so the replacement
/// survives a crash.
///
/// # Errors
/// Returns the I/O error from staging, syncing or publishing the file.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = StagedFile::write(path, bytes)?;
    std::fs::rename(&staged.path, path)?;
    staged.published()
}

/// Open a file that [`replace`] publishes, for reading.
///
/// # Errors
/// Returns the open error, including `NotFound` for a file that does not
/// exist.
pub fn open(path: &Path) -> io::Result<File> {
    File::open(path)
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
/// # Errors
/// Returns the error from opening the directory.
pub fn read_dir(directory: &Path) -> io::Result<Vec<DirEntry>> {
    let entries: Vec<DirEntry> = std::fs::read_dir(directory)?.flatten().collect();
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
/// (`renameat2` or a hard link).
///
/// # Errors
/// Returns `AlreadyExists` when `path` exists, leaving that file untouched,
/// or the I/O error from staging, syncing or publishing the file.
pub fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = StagedFile::write(path, bytes)?;
    atomicwrites::move_atomic(&staged.path, path)?;
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

fn remove_staged(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not remove a staged coordination file");
        }
    }
}

/// Flush the directory entry so a published file survives a power loss.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
