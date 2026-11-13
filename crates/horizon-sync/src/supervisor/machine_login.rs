//! The SSH login of a machine of the machine list, for a server that runs on one.
//!
//! A machine that Horizon rented holds the public key of Horizon and no key of the
//! person, so the person's own SSH configuration cannot reach it. A server entry
//! that names such a machine is therefore resolved here, from the same record and
//! the same machine key that a terminal of that machine uses, and the link starts
//! `ssh` with that login alone.
//!
//! The record comes from the mirror of the link, which holds what the server gave
//! together with what this computer wrote and did not send yet, or from the machine
//! list of the home when it has no mirror. The key comes from the key binding of
//! this computer, so a key name that only another computer holds answers one
//! sentence and no link.
//!
//! Both sources are files, so this runs on the worker that starts the link and
//! never on a frame.

use std::path::Path;

use horizon_core::machines::MachineStore as _;
use horizon_core::{Machine, MachineKeys, MachineRegistry, SshConnection};

use super::files;
use crate::error::SyncError;

/// The login of `name` on the machine list of `directory`, with the key that this
/// computer binds to it.
///
/// # Errors
/// Returns one sentence for a machine that the list of this computer does not hold,
/// one whose route the rule of a terminal refuses, and one whose key this computer
/// does not hold.
pub fn machine_login(directory: &Path, keys: &MachineKeys, name: &str) -> Result<SshConnection, SyncError> {
    let record = machine_record(directory, name)?.ok_or_else(|| {
        SyncError::Queue(format!(
            "The machine list of this computer has no machine {name}. Add it with machine_set, or name the SSH target of the server instead."
        ))
    })?;
    let terminal = record
        .checked_terminal()
        .map_err(|error| SyncError::Queue(format!("the machine {name}: {error}")))?;
    let key = match binding(directory, name)? {
        Some(key) => Some(keys.require(&key).map_err(|error| {
            SyncError::Queue(format!(
                "The key {key} that the machine {name} uses is not on this computer: {error}"
            ))
        })?),
        None => None,
    };
    Ok(match key {
        Some(key) => SshConnection::for_agent_terminal_with_key(&terminal, &key),
        None => SshConnection::for_agent_terminal(&terminal),
    })
}

/// The record of `name`: the one in the mirror of the link, which is what the
/// server gave and what this computer wrote, or the one in the machine list of the
/// home.
fn machine_record(directory: &Path, name: &str) -> Result<Option<Machine>, SyncError> {
    let mirror = files::read_mirror(&directory.join(files::MIRROR_FILE))?;
    if let Some(record) = mirror.machines.into_iter().find(|machine| machine.name == name) {
        return Ok(Some(record));
    }
    let loaded = MachineRegistry::new(directory.to_path_buf())
        .load()
        .map_err(|error| SyncError::Queue(format!("the machine list of this computer: {error}")))?;
    Ok(loaded.records.into_iter().find(|machine| machine.name == name))
}

/// The key name that this computer binds to `name`: the one in the mirror of the
/// link first, then the one in the key file of the home.
fn binding(directory: &Path, name: &str) -> Result<Option<String>, SyncError> {
    let mirror = files::read_mirror(&directory.join(files::MIRROR_FILE))?;
    if let Some(key) = mirror.bindings.get(name) {
        return Ok(Some(key.clone()));
    }
    Ok(files::read_bindings(directory)?.get(name).cloned())
}
