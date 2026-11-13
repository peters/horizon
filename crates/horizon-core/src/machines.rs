//! The machines that an agent registers with Horizon, and the terminal that a
//! click on one opens.
//!
//! A machine is the record that an agent writes when it has rented or reached a
//! host: a name, an SSH route, an optional tmux session and the provider line
//! that the agent read from its CLI. The list belongs to the person and not to a
//! workspace or a session, so it is one file under the Horizon home,
//! `machines/registry.json`, written to a temporary file and renamed. Every call
//! here reads or writes that file, so it runs on a coordination worker thread and
//! never on the UI thread.
//!
//! The key that this computer uses for a machine is not part of the record: a key
//! name resolves only in the Horizon home that made it, and the record is what
//! another computer may see. [`MachineBindings`] keeps that binding in
//! `machines/keys.json`, beside the list, under the same atomic write. A registry
//! of an older Horizon that held the key name inside a record moves it there on
//! the first load.
//!
//! Nothing here trusts a saved record: [`Machine::checked_terminal`] re-checks
//! the route and rebuilds the tmux command, and the caller re-derives the key
//! path from the key name, so a hand-edited registry cannot turn a click into a
//! different program or an ssh option.

use std::{io, path::Path};

mod bindings;
mod record;
mod registry;
#[cfg(test)]
mod tests;

pub use bindings::MachineBindings;
pub use record::{
    CheckedMachine, Machine, check, check_record, machine_key_name, machine_name, session_command, session_name,
};
pub use registry::{LoadedMachines, MachineRegistry, MachineStore, Machines};

pub use horizon_browser_control::manifest::block_machines::{
    INVALID_MACHINE_NAME, INVALID_SESSION_NAME, MACHINE_UNAVAILABLE, MAX_MACHINES, MAX_PROVIDER_CHARS,
    MAX_SESSION_CHARS, TOO_MANY_MACHINES,
};

/// Why Horizon refused or could not read a machine record.
#[derive(Debug, thiserror::Error)]
pub enum MachineError {
    /// The name is not one machine key label.
    #[error("{message}")]
    InvalidName { message: String },
    /// The tmux session name is not one plain name.
    #[error("{message}")]
    InvalidSession { message: String },
    /// The route breaks the rule of an SSH terminal.
    #[error("{message}")]
    InvalidRoute { message: String },
    /// The key name breaks the key rule.
    #[error("{message}")]
    InvalidKey { message: String },
    /// This Horizon home already keeps the most machines that it may keep.
    #[error(
        "This Horizon home already keeps the maximum of {limit} machines. Forget one in the sidebar before you add another"
    )]
    TooMany { limit: usize },
    /// The key that a record names is not there, or Horizon could not read it.
    #[error("{reason}")]
    KeyMissing { reason: String },
    /// The record is not there, or Horizon could not read or write the registry.
    #[error("{reason}")]
    Unavailable { reason: String },
}

impl MachineError {
    /// The refusal code that the tool reports for this error.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidName { .. } => INVALID_MACHINE_NAME,
            Self::InvalidSession { .. } => INVALID_SESSION_NAME,
            Self::InvalidRoute { .. } => horizon_browser_control::manifest::block_terminal::INVALID_SSH_ROUTE,
            Self::InvalidKey { .. } => horizon_browser_control::manifest::block_keys::INVALID_KEY_NAME,
            Self::TooMany { .. } => TOO_MANY_MACHINES,
            Self::KeyMissing { .. } => horizon_browser_control::manifest::block_keys::KEY_UNAVAILABLE,
            Self::Unavailable { .. } => MACHINE_UNAVAILABLE,
        }
    }
}

impl From<crate::MachineKeyError> for MachineError {
    fn from(error: crate::MachineKeyError) -> Self {
        match error {
            crate::MachineKeyError::InvalidName { message } => Self::InvalidKey { message },
            crate::MachineKeyError::TooMany { limit } => Self::TooMany { limit },
            crate::MachineKeyError::Unavailable { reason } => Self::KeyMissing { reason },
        }
    }
}

impl From<horizon_browser_control::manifest::block_terminal::TerminalRefusal> for MachineError {
    fn from(refusal: horizon_browser_control::manifest::block_terminal::TerminalRefusal) -> Self {
        Self::InvalidRoute {
            message: refusal.message,
        }
    }
}

/// The SSH login of the machine `name` of `machines`, with the machine key that
/// this computer binds to it: the address, the user, the port, the key file and
/// the known-hosts file of a terminal of that machine.
///
/// A machine that Horizon rented holds the public key of Horizon and no key of
/// the person, so a caller that must reach one of those uses this login and never
/// the SSH configuration of the person. The record and the key name are what
/// this computer holds in memory, so this reads no file and runs on any thread.
///
/// # Errors
/// One sentence for a machine that the list does not hold, and one whose route
/// the rule of an SSH terminal refuses.
pub fn login_of(
    machines: &Machines,
    keys: &crate::ssh::MachineKeys,
    name: &str,
) -> Result<crate::ssh::SshConnection, MachineError> {
    let record = machines.find(name).ok_or_else(|| {
        unavailable(format!(
            "The machine list of this computer has no machine {name}. Add it with machine_set, or name the SSH target instead."
        ))
    })?;
    let terminal = record.checked_terminal()?;
    Ok(match machines.key(&record.name) {
        Some(key) => crate::ssh::SshConnection::for_agent_terminal_with_key(&terminal, &keys.binding(key)),
        None => crate::ssh::SshConnection::for_agent_terminal(&terminal),
    })
}

/// `records` in name order, so the sidebar and `machines` report one order.
fn in_name_order(mut records: Vec<Machine>) -> Vec<Machine> {
    records.sort_by(|left, right| left.name.cmp(&right.name));
    records
}

fn unavailable(reason: String) -> MachineError {
    MachineError::Unavailable { reason }
}

/// The refusal of a document of the machine list that cannot be read or written.
fn file_error(action: &str, label: &str, path: &Path, error: &io::Error) -> MachineError {
    unavailable(format!(
        "Horizon could not {action} {label} at {}: {error}",
        path.display()
    ))
}
