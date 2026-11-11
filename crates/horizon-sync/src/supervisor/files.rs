//! The private files of one computer that follows a server: its device name, the
//! mirror of the machine list that the server gave, and the records of the list
//! that this computer kept before it followed a server.
//!
//! Every file is in the machine directory of the Horizon home of this computer,
//! at 0600 in a 0700 directory, and every one of them is written as a whole
//! through a temporary file and a rename. The queue file of this computer is not
//! here: [`crate::client`] owns it, with the mark of the log.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use horizon_core::machines::{MachineRegistry, MachineStore};
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
/// The format of the mirror file. A build refuses a file of another format.
pub(super) const MIRROR_FORMAT: u32 = 1;

/// One machine of the mirror of this computer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct MirroredMachine {
    pub(super) record: Machine,
    /// Whether this computer wrote the record itself at some time, so it is not
    /// one that only arrived from the server.
    pub(super) mine: bool,
}

/// The mirror file: the records that the server last gave, and the names that
/// this computer wrote.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct MirrorFile {
    pub(super) format: u32,
    /// Whether this computer compared its own list with the server once, so no
    /// record of the machine list that it kept before that is lost.
    pub(super) seeded: bool,
    pub(super) owned: BTreeSet<String>,
    pub(super) machines: Vec<Machine>,
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

/// The device name of this computer: the one that it wrote at its first link, or
/// a name that it writes now. The name is 1 to 40 characters of `a-z`, `0-9` and
/// `-`, and it stays the same across restarts, so the server sees one computer.
pub(super) fn device_name(directory: &Path) -> Result<String, SyncError> {
    let file = AtomicFile::new(directory.join(DEVICE_FILE));
    if let Some(text) = file
        .read()
        .map_err(|error| SyncError::Queue(format!("the device name at {}: {error}", file.path().display())))?
    {
        let name = text.trim().to_owned();
        if check_device_name(&name) {
            return Ok(name);
        }
        tracing::warn!(
            name = %name,
            "the stored device name breaks its rule, so this computer takes another one"
        );
    }
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("horizon-{}", &unique[..8]);
    file.write(name.as_bytes())
        .map_err(|error| SyncError::Queue(format!("the device name at {}: {error}", file.path().display())))?;
    Ok(name)
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
