//! One private file of a Horizon home, read whole and replaced whole.
//!
//! A reader sees either the old document or the new one: the write goes to a
//! temporary file in the same directory, is synced, and is renamed over the
//! document. The directory and the file are private, because they hold hosts,
//! keys and records of the person. A caller reads and writes this file on a
//! worker thread, never on the UI thread.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// The mode of a directory that holds a private file.
pub const DIRECTORY_MODE: u32 = 0o700;

/// The mode of a private file.
pub const FILE_MODE: u32 = 0o600;

/// One document that Horizon reads and replaces as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtomicFile {
    path: PathBuf,
}

impl AtomicFile {
    /// The document at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The path of the document.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text of the document now, or `None` when there is no file yet.
    ///
    /// # Errors
    /// Returns why the document cannot be read.
    pub fn read(&self) -> io::Result<Option<String>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Replaces the document with `encoded`, through a temporary file and a
    /// rename.
    ///
    /// # Errors
    /// Returns why the directory or the document cannot be written.
    pub fn write(&self, encoded: &[u8]) -> io::Result<()> {
        crate::ui_thread::blocking("a private file is replaced and synced");
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        private_directory(directory)?;
        let temporary = self.path.with_extension("json.tmp");
        let written = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&temporary)
            .and_then(|mut file| {
                use io::Write as _;
                file.write_all(encoded)?;
                file.sync_all()
            })
            .and_then(|()| fs::rename(&temporary, &self.path));
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }
}

/// Makes `directory` private, creating it when it is not there.
fn private_directory(directory: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY_MODE)
        .create(directory)
        .and_then(|()| fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE)))
}
