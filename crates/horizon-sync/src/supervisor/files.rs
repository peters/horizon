//! The private files of one computer that follows a server: its device name, the
//! mirror of the machine list that the server gave, and the records of the list
//! that this computer kept before it followed a server.
//!
//! Every file is in the machine directory of the Horizon home of this computer,
//! at 0600 in a 0700 directory, and every one of them is written as a whole
//! through a temporary file and a rename. The queue file of this computer is not
//! here: [`crate::client`] owns it, with the mark of the log.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use horizon_core::machines::{MachineBindings, MachineRegistry, MachineStore};
use horizon_core::{AtomicFile, Machine};
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::protocol::check_device_name;

/// The file of the device name of this computer, in its machine directory.
pub(super) const DEVICE_FILE: &str = "server-device";
/// The file of the queue, in the same directory.
pub(super) const QUEUE_FILE: &str = "server-queue.json";
/// The file of the mirror, in the same directory.
pub(super) const MIRROR_FILE: &str = "server-mirror.json";
/// The file of the mirror of the settings, in the same directory. The settings of
/// a computer have their own file, so a build that reads no setting reads the
/// machine list of a newer one as it always did.
pub(super) const SETTINGS_MIRROR_FILE: &str = "server-settings.json";
/// The format of the settings mirror. A build refuses a file of another format.
pub(super) const SETTINGS_MIRROR_FORMAT: u32 = 1;
/// The format of the mirror file. A build refuses a file of another format.
pub(super) const MIRROR_FORMAT: u32 = 1;
/// The first characters of a device name that a computer proposes for itself.
pub(super) const DEVICE_PREFIX: &str = "horizon-";

/// One machine of the mirror of this computer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct MirroredMachine {
    pub(super) record: Machine,
    /// Whether this computer wrote the record itself at some time, so it is not
    /// one that only arrived from the server.
    pub(super) mine: bool,
}

/// The mirror file: the records that the server last gave, the key that each
/// machine uses, and the names that this computer wrote.
///
/// The mirror holds no secret body: a value of the server is written where this
/// computer reads it, in the key folder or the credential store.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct MirrorFile {
    pub(super) format: u32,
    /// Whether this computer compared its own list and its own keys with the
    /// server once, so nothing that it kept before that link is lost.
    pub(super) seeded: bool,
    pub(super) owned: BTreeSet<String>,
    pub(super) machines: Vec<Machine>,
    /// The key that each machine uses, as the server gave it.
    pub(super) bindings: BTreeMap<String, String>,
}

/// One section of the settings that the server last gave, and its version.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct MirroredSetting {
    pub(super) version: u64,
    pub(super) body: serde_json::Value,
}

/// The mirror of the settings: the section of the server that this computer
/// applied, the version of the store that it holds, and whether it compared its
/// own settings with the server once.
///
/// `applied` is what makes a restart safe: a section that the store already holds
/// is not applied again, so a change of this computer that waits for an away
/// server is not replaced by the older row of the server.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SettingsMirror {
    pub(super) format: u32,
    pub(super) seeded: bool,
    pub(super) applied: BTreeMap<String, u64>,
    pub(super) rows: BTreeMap<String, MirroredSetting>,
}

/// The records of the machine list that this computer kept before it followed a
/// server, from the file store of `machines_dir`. A key name that such a record
/// still holds moves to the binding file of this computer, as a load of the file
/// store moves it.
pub(super) fn seed_records(machines_dir: &Path) -> Result<Vec<Machine>, SyncError> {
    let registry = MachineRegistry::new(machines_dir.to_path_buf());
    let loaded = registry
        .load()
        .map_err(|error| SyncError::Queue(format!("the machine list of this computer: {error}")))?;
    if !loaded.keys.is_empty() {
        let mut bindings = registry
            .bindings()
            .load()
            .map_err(|error| SyncError::Queue(format!("the machine keys of this computer: {error}")))?;
        let moved = loaded.keys.iter().fold(false, |moved, (machine, key)| {
            bindings.bind_if_absent(machine, key) || moved
        });
        if moved {
            bindings
                .save()
                .map_err(|error| SyncError::Queue(format!("the machine keys of this computer: {error}")))?;
        }
    }
    Ok(loaded.records)
}

/// The key bindings of this computer, from `machines/keys.json`.
pub(super) fn read_bindings(machines_dir: &Path) -> Result<BTreeMap<String, String>, SyncError> {
    let bindings = MachineBindings::in_directory(machines_dir)
        .load()
        .map_err(|error| SyncError::Queue(format!("the machine keys of this computer: {error}")))?;
    Ok(bindings.names().clone())
}

/// The device name of this computer: the one that it wrote at its first link, or
/// a name that it writes now. The name is 1 to 40 characters of `a-z`, `0-9` and
/// `-`, and it stays the same across restarts, so the server sees one computer.
pub(super) fn device_name(directory: &Path) -> Result<String, SyncError> {
    if let Some(name) = stored_device_name(directory)? {
        return Ok(name);
    }
    let name = proposed_device_name();
    write_device_name(directory, &name)?;
    tracing::info!(name = %name, "this computer named itself, because its machine directory held no name");
    Ok(name)
}

/// The device name that this computer wrote, or `None` when its machine directory
/// holds no name that passes the rule. This call reads the file and writes
/// nothing.
pub(super) fn stored_device_name(directory: &Path) -> Result<Option<String>, SyncError> {
    let file = AtomicFile::new(directory.join(DEVICE_FILE));
    let Some(text) = file
        .read()
        .map_err(|error| SyncError::Queue(format!("the device name at {}: {error}", file.path().display())))?
    else {
        return Ok(None);
    };
    let name = text.trim().to_owned();
    if check_device_name(&name) {
        return Ok(Some(name));
    }
    tracing::warn!(name = %name, "the stored device name breaks its rule, so this computer reads no name");
    Ok(None)
}

/// Writes `name` as the device name of this computer.
///
/// A join writes this file before it takes the token of its server, so the name of
/// the join is the name that every later link of this computer presents.
pub(super) fn write_device_name(directory: &Path, name: &str) -> Result<(), SyncError> {
    if !check_device_name(name) {
        return Err(SyncError::Device(name.to_owned()));
    }
    let file = AtomicFile::new(directory.join(DEVICE_FILE));
    file.write(name.as_bytes())
        .map_err(|error| SyncError::Queue(format!("the device name at {}: {error}", file.path().display())))
}

/// A name that a computer without a login takes: [`DEVICE_PREFIX`] and eight
/// characters of random, so two fresh computers do not claim one name.
pub(super) fn proposed_device_name() -> String {
    let unique = uuid::Uuid::new_v4().simple().to_string();
    format!("{DEVICE_PREFIX}{}", &unique[..8])
}

pub(super) fn read_mirror(path: &Path) -> Result<MirrorFile, SyncError> {
    let file = AtomicFile::new(PathBuf::from(path));
    let Some(text) = file
        .read()
        .map_err(|error| SyncError::Queue(format!("the mirror at {}: {error}", path.display())))?
    else {
        return Ok(MirrorFile::default());
    };
    let mirror: MirrorFile = serde_json::from_str(&text)
        .map_err(|error| SyncError::Queue(format!("the mirror at {}: {error}", path.display())))?;
    if mirror.format != 0 && mirror.format != MIRROR_FORMAT {
        return Err(SyncError::Queue(format!(
            "the mirror at {} has format {}, and this Horizon writes format {MIRROR_FORMAT}",
            path.display(),
            mirror.format
        )));
    }
    Ok(mirror)
}

pub(super) fn write_mirror(path: &Path, file: &MirrorFile) -> Result<(), SyncError> {
    let encoded = serde_json::to_vec_pretty(file)
        .map_err(|error| SyncError::Queue(format!("the mirror at {}: {error}", path.display())))?;
    AtomicFile::new(PathBuf::from(path))
        .write(&encoded)
        .map_err(|error| SyncError::Queue(format!("the mirror at {}: {error}", path.display())))
}

pub(super) fn read_settings_mirror(path: &Path) -> Result<SettingsMirror, SyncError> {
    let file = AtomicFile::new(PathBuf::from(path));
    let Some(text) = file
        .read()
        .map_err(|error| SyncError::Queue(format!("the settings mirror at {}: {error}", path.display())))?
    else {
        return Ok(SettingsMirror::default());
    };
    let mirror: SettingsMirror = serde_json::from_str(&text)
        .map_err(|error| SyncError::Queue(format!("the settings mirror at {}: {error}", path.display())))?;
    if mirror.format != 0 && mirror.format != SETTINGS_MIRROR_FORMAT {
        return Err(SyncError::Queue(format!(
            "the settings mirror at {} has format {}, and this Horizon writes format {SETTINGS_MIRROR_FORMAT}",
            path.display(),
            mirror.format
        )));
    }
    Ok(mirror)
}

pub(super) fn write_settings_mirror(path: &Path, mirror: &SettingsMirror) -> Result<(), SyncError> {
    let encoded = serde_json::to_vec_pretty(mirror)
        .map_err(|error| SyncError::Queue(format!("the settings mirror at {}: {error}", path.display())))?;
    AtomicFile::new(PathBuf::from(path))
        .write(&encoded)
        .map_err(|error| SyncError::Queue(format!("the settings mirror at {}: {error}", path.display())))
}
