//! The machines that an agent registers with Horizon, and the terminal that a
//! click on one opens.
//!
//! A machine is the record that an agent writes when it has rented or reached a
//! host: a name, an SSH route, the machine key that Horizon keeps for it, an
//! optional tmux session and the provider line that the agent read from its CLI.
//! The list belongs to the person and not to a workspace or a session, so it is
//! one file under the Horizon home, `machines/registry.json`, written to a
//! temporary file and renamed. Every call here reads or writes that file, so it
//! runs on a coordination worker thread and never on the UI thread.
//!
//! Nothing here trusts a saved record: [`Machine::checked_terminal`] re-checks
//! the route and rebuilds the tmux command, and the caller re-derives the key
//! path from the key name, so a hand-edited registry cannot turn a click into a
//! different program or an ssh option.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_terminal::{self, CheckedTerminal, TerminalRoute};
use serde::{Deserialize, Serialize};

use crate::ssh::{MachineKeyRef, SshConnection};

pub use horizon_browser_control::manifest::block_machines::{
    INVALID_MACHINE_NAME, INVALID_SESSION_NAME, MACHINE_UNAVAILABLE, MAX_MACHINES, MAX_PROVIDER_CHARS,
    MAX_SESSION_CHARS, TOO_MANY_MACHINES,
};

/// The mode of the directory that holds the registry.
const DIRECTORY_MODE: u32 = 0o700;

/// The mode of the registry file. It names hosts, and a host is the person's.
const FILE_MODE: u32 = 0o600;

/// One machine that an agent registered with Horizon.
///
/// The name follows the machine key rule and is unique in the list. The route is
/// re-checked with the rule of `open_terminal` every time Horizon uses it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Machine {
    pub name: String,
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The name of a machine key of this Horizon home, when one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The tmux session that the terminal attaches to, when one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The provider line as the agent reported it, such as `Hetzner, hel1`. It
    /// is the agent's claim, not verified Horizon state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub added_at_millis: u64,
}

/// A machine whose name, route, key, session and provider line passed their
/// checks: the only values that a record is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedMachine {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub key: Option<String>,
    pub session: Option<String>,
    pub provider: Option<String>,
}

impl CheckedMachine {
    /// The record of this machine, added at `added_at_millis`.
    #[must_use]
    pub fn record(&self, added_at_millis: u64) -> Machine {
        Machine {
            name: self.name.clone(),
            host: self.host.clone(),
            user: self.user.clone(),
            port: self.port,
            key: self.key.clone(),
            session: self.session.clone(),
            provider: self.provider.clone(),
            added_at_millis,
        }
    }
}

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
            Self::InvalidRoute { .. } => block_terminal::INVALID_SSH_ROUTE,
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

impl From<block_terminal::TerminalRefusal> for MachineError {
    fn from(refusal: block_terminal::TerminalRefusal) -> Self {
        Self::InvalidRoute {
            message: refusal.message,
        }
    }
}

/// Check one machine record from an agent: its name, route, key name, tmux
/// session and provider line.
///
/// # Errors
/// Describes the first refused field with the code of [`MachineError::code`].
pub fn check(
    name: &str,
    route: &TerminalRoute,
    key: Option<&str>,
    session: Option<&str>,
    provider: Option<&str>,
) -> Result<CheckedMachine, MachineError> {
    let name = machine_name(name)?;
    let terminal = block_terminal::check(route, None)?;
    let key = key.map(key_name).transpose()?;
    let session = session.map(session_name).transpose()?;
    let provider = provider.map(block_machines::provider_line);
    Ok(CheckedMachine {
        name: name.to_owned(),
        host: terminal.host,
        user: terminal.user,
        port: terminal.port,
        key,
        session,
        provider,
    })
}

/// Check one machine name. It is the key name rule, so a record and its key
/// pair always have one name.
///
/// # Errors
/// Describes the rule with [`INVALID_MACHINE_NAME`].
pub fn machine_name(name: &str) -> Result<&str, MachineError> {
    block_machines::check_machine_name(name).map_err(|refusal| MachineError::InvalidName {
        message: refusal.message,
    })
}

/// Check one tmux session name.
///
/// # Errors
/// Describes the rule with [`INVALID_SESSION_NAME`].
pub fn session_name(session: &str) -> Result<String, MachineError> {
    block_machines::check_session_name(session)
        .map(str::to_owned)
        .map_err(|refusal| MachineError::InvalidSession {
            message: refusal.message,
        })
}

/// Check one key name of a record. The key stays optional: a machine without a
/// key connects with the person's own keys.
fn key_name(key: &str) -> Result<String, MachineError> {
    horizon_browser_control::manifest::block_keys::check_name(key)
        .map(str::to_owned)
        .map_err(|refusal| MachineError::InvalidKey {
            message: refusal.message,
        })
}

impl Machine {
    /// The terminal that this machine's connection asks for, re-built from the
    /// record: the route is checked again and the tmux command is Horizon's own
    /// text, never a saved command line.
    ///
    /// # Errors
    /// Refuses a route or a session name that no longer passes its rule.
    pub fn checked_terminal(&self) -> Result<CheckedTerminal, MachineError> {
        let route = TerminalRoute {
            host: self.host.clone(),
            user: self.user.clone(),
            port: self.port.map(i64::from),
        };
        let session = self.session.as_deref().map(session_name).transpose()?;
        block_terminal::check(
            &route,
            session.map(|session| format!("tmux new -A -s {session}")).as_deref(),
        )
        .map_err(MachineError::from)
    }

    /// The `ssh` command line of a click on this machine, with the key `key`.
    ///
    /// # Errors
    /// Refuses a record that no longer passes its rule.
    pub fn ssh_args(&self, key: &MachineKeyRef) -> Result<Vec<String>, MachineError> {
        Ok(SshConnection::for_agent_terminal_with_key(&self.checked_terminal()?, key).to_command_args())
    }

    /// The title of a terminal that this machine opens.
    #[must_use]
    pub fn title(&self) -> String {
        self.name.clone()
    }
}

/// The registry file of one Horizon home: `machines/registry.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRegistry {
    root: PathBuf,
}

impl MachineRegistry {
    /// The registry of `home`.
    #[must_use]
    pub fn from_home(home: &crate::horizon_home::HorizonHome) -> Self {
        Self::new(home.machines_dir())
    }

    /// The registry of a directory that holds it, for a caller that resolved one.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The file that holds the records.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root.join("registry.json")
    }

    /// The records of this home, in name order.
    ///
    /// # Errors
    /// Returns why the file cannot be read or parsed.
    pub fn load(&self) -> Result<Vec<Machine>, MachineError> {
        let path = self.path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(unavailable(format!(
                    "Horizon could not read the machine list at {}: {error}",
                    path.display()
                )));
            }
        };
        let file: RegistryFile = serde_json::from_str(&text).map_err(|error| {
            unavailable(format!(
                "The machine list at {} is not valid JSON: {error}",
                path.display()
            ))
        })?;
        Ok(in_name_order(file.machines))
    }
}

impl Machines {
    /// The records of `registry`, read now.
    ///
    /// # Errors
    /// Returns why the registry cannot be read or parsed.
    pub fn load(registry: MachineRegistry) -> Result<Self, MachineError> {
        let records = registry.load()?;
        Ok(Self { records, registry })
    }

    /// A list that holds `records`, with no file read.
    #[must_use]
    pub fn with_records(registry: MachineRegistry, records: Vec<Machine>) -> Self {
        Self {
            records: in_name_order(records),
            registry,
        }
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

    /// The directory that holds the registry.
    #[must_use]
    pub fn registry(&self) -> &MachineRegistry {
        &self.registry
    }

    /// This list with `record` added or replaced by its name, and whether the
    /// name is new.
    ///
    /// # Errors
    /// Refuses a name that is new while this home keeps the most machines.
    pub fn with_record(&self, record: Machine) -> Result<(Self, bool), MachineError> {
        let existing = self.records.iter().position(|machine| machine.name == record.name);
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
        Ok((
            Self {
                records: in_name_order(records),
                registry: self.registry.clone(),
            },
            created,
        ))
    }

    /// This list without the record of `name`.
    #[must_use]
    pub fn without_record(&self, name: &str) -> Self {
        Self {
            records: self
                .records
                .iter()
                .filter(|machine| machine.name != name)
                .cloned()
                .collect(),
            registry: self.registry.clone(),
        }
    }

    /// Write this list to the registry, through a temporary file and a rename so
    /// a reader sees either the old list or the new one.
    ///
    /// # Errors
    /// Returns why the directory or the file cannot be written.
    pub fn save(&self) -> Result<(), MachineError> {
        let directory = &self.registry.root;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIRECTORY_MODE)
            .create(directory)
            .and_then(|()| fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE)))
            .map_err(|error| {
                unavailable(format!(
                    "Horizon could not make {} private: {error}",
                    directory.display()
                ))
            })?;
        let path = self.registry.path();
        let temporary = path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(&RegistryFile {
            machines: self.records.clone(),
        })
        .map_err(|error| unavailable(format!("Horizon could not encode the machine list: {error}")))?;
        let written = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&temporary)
            .and_then(|mut file| {
                use std::io::Write as _;
                file.write_all(&encoded)?;
                file.sync_all()
            })
            .and_then(|()| fs::rename(&temporary, &path));
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(unavailable(format!(
                "Horizon could not write the machine list at {}: {error}",
                path.display()
            )));
        }
        Ok(())
    }
}

/// The machines of one Horizon home, as the host holds them now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machines {
    records: Vec<Machine>,
    registry: MachineRegistry,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RegistryFile {
    machines: Vec<Machine>,
}

/// `records` in name order, so the sidebar and `machines` report one order.
fn in_name_order(mut records: Vec<Machine>) -> Vec<Machine> {
    records.sort_by(|left, right| left.name.cmp(&right.name));
    records
}

fn unavailable(reason: String) -> MachineError {
    MachineError::Unavailable { reason }
}

#[cfg(test)]
mod tests;
