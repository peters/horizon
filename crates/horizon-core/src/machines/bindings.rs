//! The key name that this computer uses for each machine: `machines/keys.json`.
//!
//! A machine key is private to the Horizon home that made it, so the name of a
//! key is not part of a machine record: the record may travel to another
//! computer, and a key name would name a key that is not there. This file keeps
//! the binding on this computer alone, beside the registry and under the same
//! rules: it is written to a temporary file, synced and renamed, so no frame ever
//! waits for the disk.

use std::{collections::BTreeMap, path::Path};

use super::{MachineError, file_error, unavailable};
use crate::AtomicFile;

/// The name of the binding file inside the machine directory.
const FILE_NAME: &str = "keys.json";

/// The label of the file in a refusal.
const LABEL: &str = "the machine key bindings";

/// The machine names of one Horizon home, each with the key that this computer
/// uses for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineBindings {
    file: AtomicFile,
    names: BTreeMap<String, String>,
}

impl MachineBindings {
    /// The bindings in the directory that holds the machine list, with no file
    /// read.
    #[must_use]
    pub fn in_directory(directory: &Path) -> Self {
        Self {
            file: AtomicFile::new(directory.join(FILE_NAME)),
            names: BTreeMap::new(),
        }
    }

    /// The path of the binding file.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// The bindings in the file, read now. A file that is not there is an empty
    /// set of bindings.
    ///
    /// # Errors
    /// Returns why the file cannot be read or parsed.
    pub fn load(&self) -> Result<Self, MachineError> {
        let Some(text) = self
            .file
            .read()
            .map_err(|error| file_error("read", LABEL, self.path(), &error))?
        else {
            return Ok(Self {
                file: self.file.clone(),
                names: BTreeMap::new(),
            });
        };
        let names: BTreeMap<String, String> = serde_json::from_str(&text).map_err(|error| {
            unavailable(format!(
                "The machine key bindings at {} are not valid JSON: {error}",
                self.path().display()
            ))
        })?;
        Ok(Self {
            file: self.file.clone(),
            names,
        })
    }

    /// The key that this computer uses for `machine`.
    #[must_use]
    pub fn key(&self, machine: &str) -> Option<&str> {
        self.names.get(machine).map(String::as_str)
    }

    /// Every binding, by machine name.
    #[must_use]
    pub fn names(&self) -> &BTreeMap<String, String> {
        &self.names
    }

    /// Sets the key of `machine`, or forgets it when `key` is `None`, and
    /// returns whether the binding changed.
    pub fn set(&mut self, machine: &str, key: Option<&str>) -> bool {
        match key {
            Some(key) => self.names.insert(machine.to_owned(), key.to_owned()) != Some(key.to_owned()),
            None => self.names.remove(machine).is_some(),
        }
    }

    /// Sets the key of `machine` unless the file already binds another key to it,
    /// and returns whether the binding changed. A record of an older Horizon that
    /// still held a key name moves here; a binding that is already there wins,
    /// because this computer made it.
    pub fn bind_if_absent(&mut self, machine: &str, key: &str) -> bool {
        if self.names.contains_key(machine) {
            return false;
        }
        self.names.insert(machine.to_owned(), key.to_owned());
        true
    }

    /// Replaces the file with these bindings.
    ///
    /// # Errors
    /// Returns why the directory or the file cannot be written.
    pub fn save(&self) -> Result<(), MachineError> {
        let encoded = serde_json::to_vec_pretty(&self.names)
            .map_err(|error| unavailable(format!("Horizon could not encode {LABEL}: {error}")))?;
        self.file
            .write(&encoded)
            .map_err(|error| file_error("write", LABEL, self.path(), &error))
    }
}
