//! One private JSON file under the Horizon home, read whole and replaced whole.
//!
//! A reader of the machine list sees either the old document or the new one: the
//! write goes to a temporary file in the same directory, is synced, and is
//! renamed over the document. The directory is private and the file is private,
//! because both name hosts of the person.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{MachineError, unavailable};

/// The mode of a directory that holds a file of the machine list.
const DIRECTORY_MODE: u32 = 0o700;

/// The mode of a file that names hosts. It is the person's, not the server's.
const FILE_MODE: u32 = 0o600;

/// One document that Horizon reads and replaces as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtomicFile {
    path: PathBuf,
    label: &'static str,
}

impl AtomicFile {
    /// The document at `path`, named `label` in a refusal.
    #[must_use]
    pub fn new(path: PathBuf, label: &'static str) -> Self {
        Self { path, label }
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
    pub fn read(&self) -> Result<Option<String>, MachineError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable(format!(
                "Horizon could not read {} at {}: {error}",
                self.label,
                self.path.display()
            ))),
        }
    }

    /// Replaces the document with `encoded`, through a temporary file and a
    /// rename.
    ///
    /// # Errors
    /// Returns why the directory or the document cannot be written.
    pub fn write(&self, encoded: &[u8]) -> Result<(), MachineError> {
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        private_directory(directory, self.label)?;
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
            return Err(unavailable(format!(
                "Horizon could not write {} at {}: {error}",
                self.label,
                self.path.display()
            )));
        }
        Ok(())
    }
}

/// Makes `directory` private, creating it when it is not there.
fn private_directory(directory: &Path, label: &'static str) -> Result<(), MachineError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY_MODE)
        .create(directory)
        .and_then(|()| fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE)))
        .map_err(|error| {
            unavailable(format!(
                "Horizon could not make the directory of {label} at {} private: {error}",
                directory.display()
            ))
        })
}
