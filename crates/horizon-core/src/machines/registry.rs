//! Where the shareable machine records live: `machines/registry.json` of the
//! Horizon home, behind [`MachineStore`].
//!
//! The store keeps records only. The key that this computer uses for a machine
//! is a local binding in [`super::MachineBindings`], and a record of an older
//! Horizon that still held the name of a key is moved there when the list is
//! read: the binding file is written first, then the registry without the field.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use super::{MAX_MACHINES, Machine, MachineBindings, MachineError, file_error, in_name_order, unavailable};
use crate::AtomicFile;

/// The label of the registry in a refusal.
const LABEL: &str = "the machine list";

/// Where the shareable machine records live.
///
/// One implementation keeps them in a file of the Horizon home. A later slice
/// keeps them on a server, and only the store and its caller change: [`Machines`]
/// loads and saves through this trait.
pub trait MachineStore: std::fmt::Debug + Send + Sync {
    /// Every record of this store, in name order, and the key names that a store
    /// of an older Horizon kept inside a record.
    ///
    /// # Errors
    /// Returns why the store cannot be read or parsed.
    fn load(&self) -> Result<LoadedMachines, MachineError>;

    /// Replaces every record of this store with `records`.
    ///
    /// # Errors
    /// Returns why the store cannot be written.
    fn save(&self, records: &[Machine]) -> Result<(), MachineError>;

    /// Whether [`Self::load`] answers the whole set of key bindings of the person.
    ///
    /// A store that a server holds does, and a caller may then make the binding
    /// file of this computer equal to that answer. The file store holds the
    /// bindings itself and answers only the names that a record of an older Horizon
    /// still held, so a caller fills a gap there and never replaces the set.
    #[must_use]
    fn holds_bindings(&self) -> bool {
        false
    }

    /// Writes the key binding of this computer to where the store keeps it. The
    /// file store already holds it beside the records, so it writes nothing here;
    /// a store that a server holds turns the difference into intents.
    ///
    /// # Errors
    /// Returns why the bindings cannot be written.
    fn save_bindings(&self, bindings: &MachineBindings) -> Result<(), MachineError> {
        let _ = bindings;
        Ok(())
    }
}

/// What a store returned: its records, and the key names that a record still
/// held in a layout that had no binding file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedMachines {
    pub records: Vec<Machine>,
    /// Machine name and key name, from a record that still held one.
    pub keys: Vec<(String, String)>,
}

/// The registry file of one Horizon home: `machines/registry.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRegistry {
    file: AtomicFile,
}

impl MachineRegistry {
    /// The registry of `home`.
    #[must_use]
    pub fn from_home(home: &crate::horizon_home::HorizonHome) -> Self {
        Self::new(home.machines_dir())
    }

    /// The registry of a directory that holds it, for a caller that resolved one.
    #[must_use]
    pub fn new(mut root: PathBuf) -> Self {
        root.push("registry.json");
        Self {
            file: AtomicFile::new(root),
        }
    }

    /// The file that holds the records.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.file.path().to_path_buf()
    }

    /// The directory that holds the registry and the key bindings.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.file.path().parent().unwrap_or_else(|| Path::new("."))
    }

    /// The key bindings of this computer, in the directory of this registry.
    #[must_use]
    pub fn bindings(&self) -> MachineBindings {
        MachineBindings::in_directory(self.root())
    }
}

impl MachineStore for MachineRegistry {
    fn load(&self) -> Result<LoadedMachines, MachineError> {
        let Some(text) = self
            .file
            .read()
            .map_err(|error| file_error("read", LABEL, self.file.path(), &error))?
        else {
            return Ok(LoadedMachines::default());
        };
        let mut document: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            unavailable(format!(
                "The machine list at {} is not valid JSON: {error}",
                self.path().display()
            ))
        })?;
        let keys = take_keys(&mut document);
        let file: RegistryFile = serde_json::from_value(document).map_err(|error| {
            unavailable(format!(
                "The machine list at {} is not valid JSON: {error}",
                self.path().display()
            ))
        })?;
        Ok(LoadedMachines {
            records: in_name_order(file.machines),
            keys,
        })
    }

    fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        let encoded = serde_json::to_vec_pretty(&RegistryFile {
            machines: records.to_vec(),
        })
        .map_err(|error| unavailable(format!("Horizon could not encode {LABEL}: {error}")))?;
        self.file
            .write(&encoded)
            .map_err(|error| file_error("write", LABEL, self.file.path(), &error))
    }
}

/// The machines of one Horizon home, as the host holds them now: the records of
/// the store and the key bindings of this computer.
#[derive(Clone, Debug)]
pub struct Machines {
    records: Vec<Machine>,
    bindings: MachineBindings,
    store: Arc<dyn MachineStore>,
}

impl Machines {
    /// The records of `registry` and the key bindings beside it, read now.
    ///
    /// A record that still holds the name of a key, as a registry of an older
    /// Horizon does, moves that name to the binding file; the registry is then
    /// written once without the field.
    ///
    /// # Errors
    /// Returns why the registry or the binding file cannot be read or written.
    pub fn load(registry: MachineRegistry) -> Result<Self, MachineError> {
        let loaded = registry.load()?;
        let mut bindings = registry.bindings().load()?;
        if !loaded.keys.is_empty() {
            let moved = loaded.keys.iter().fold(false, |moved, (machine, key)| {
                bindings.bind_if_absent(machine, key) || moved
            });
            if moved {
                bindings.save()?;
                registry.save(&loaded.records)?;
            }
        }
        Ok(Self {
            records: in_name_order(loaded.records),
            bindings,
            store: Arc::new(registry),
        })
    }

    /// A list that holds `records`, with no file read and no key binding.
    #[must_use]
    pub fn with_records(registry: MachineRegistry, records: Vec<Machine>) -> Self {
        Self {
            records: in_name_order(records),
            bindings: registry.bindings(),
            store: Arc::new(registry),
        }
    }

    /// The records of `store` and the key bindings of this computer beside
    /// `registry`, read now.
    ///
    /// A store that a server holds answers the whole set of key bindings, and this
    /// computer takes that set: the key of a machine is the key of the server, and a
    /// binding that the server removed goes from the file of this computer. The
    /// file keeps them, so a click reads a binding without a read of the server. A
    /// file store answers only the names of an older record, so a caller fills a gap
    /// there.
    ///
    /// # Errors
    /// Returns why the store or the binding file cannot be read or written.
    pub fn from_store(registry: &MachineRegistry, store: Arc<dyn MachineStore>) -> Result<Self, MachineError> {
        let loaded = store.load()?;
        let mut bindings = registry.bindings().load()?;
        let mut changed = false;
        if store.holds_bindings() {
            let wanted: std::collections::BTreeMap<String, String> = loaded.keys.iter().cloned().collect();
            for machine in bindings.names().clone().keys() {
                if !wanted.contains_key(machine) {
                    changed |= bindings.set(machine, None);
                }
            }
            for (machine, key) in &wanted {
                changed |= bindings.set(machine, Some(key));
            }
        } else {
            for (machine, key) in &loaded.keys {
                changed |= bindings.bind_if_absent(machine, key);
            }
        }
        if changed {
            bindings.save()?;
        }
        Ok(Self {
            records: in_name_order(loaded.records),
            bindings,
            store,
        })
    }

    /// This list with the key `key` bound to `machine` on this computer.
    #[must_use]
    pub fn with_key(mut self, machine: &str, key: &str) -> Self {
        self.bindings.set(machine, Some(key));
        self
    }

    /// The records, in name order.
    #[must_use]
    pub fn records(&self) -> &[Machine] {
        &self.records
    }

    /// The record of `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Machine> {
        self.records.iter().find(|machine| machine.name == name)
    }

    /// The key that this computer uses for `machine`.
    #[must_use]
    pub fn key(&self, machine: &str) -> Option<&str> {
        self.bindings.key(machine)
    }

    /// The key bindings of this computer.
    #[must_use]
    pub fn bindings(&self) -> &MachineBindings {
        &self.bindings
    }

    /// This list with `record` added or replaced by its name, and whether the
    /// name is new. `key` is the key that this computer uses for it; `None`
    /// forgets the key of a record that is replaced.
    ///
    /// # Errors
    /// Refuses a name that is new while this home keeps the most machines.
    pub fn with_record(&self, record: Machine, key: Option<&str>) -> Result<(Self, bool), MachineError> {
        let name = record.name.clone();
        let existing = self.records.iter().position(|machine| machine.name == name);
        if existing.is_none() && self.records.len() >= MAX_MACHINES {
            return Err(MachineError::TooMany { limit: MAX_MACHINES });
        }
        let mut records = self.records.clone();
        let created = if let Some(index) = existing {
            let added_at_millis = records[index].added_at_millis;
            records[index] = Machine {
                added_at_millis,
                ..record
            };
            false
        } else {
            records.push(record);
            true
        };
        let mut bindings = self.bindings.clone();
        bindings.set(&name, key);
        Ok((
            Self {
                records: in_name_order(records),
                bindings,
                store: Arc::clone(&self.store),
            },
            created,
        ))
    }

    /// This list without the record of `name`. The key that this computer uses
    /// for the machine stays, as the key pair of the machine stays.
    #[must_use]
    pub fn without_record(&self, name: &str) -> Self {
        Self {
            records: self
                .records
                .iter()
                .filter(|machine| machine.name != name)
                .cloned()
                .collect(),
            bindings: self.bindings.clone(),
            store: Arc::clone(&self.store),
        }
    }

    /// Write the key bindings and then the records, so a machine that is written
    /// never appears without the key that this computer uses for it.
    ///
    /// # Errors
    /// Returns why the binding file or the store cannot be written.
    pub fn save(&self) -> Result<(), MachineError> {
        self.bindings.save()?;
        self.store.save_bindings(&self.bindings)?;
        self.store.save(&self.records)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RegistryFile {
    machines: Vec<Machine>,
}

/// Removes the `key` field of every record and returns the name pairs that it
/// held, so a registry of an older Horizon reads as records plus bindings.
fn take_keys(document: &mut serde_json::Value) -> Vec<(String, String)> {
    let Some(records) = document.get_mut("machines").and_then(serde_json::Value::as_array_mut) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for record in records {
        let Some(fields) = record.as_object_mut() else {
            continue;
        };
        let Some(key) = fields
            .remove("key")
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(name) = fields.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        keys.push((name.to_owned(), key));
    }
    keys
}
