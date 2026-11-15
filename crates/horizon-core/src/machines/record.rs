//! The shareable part of a machine: its name, its route, the tmux session that a
//! terminal attaches to, the repository that the machine is ready for and the
//! provider line that the agent reported.
//!
//! A record is what another computer may see, so it holds no name of a key: a key
//! name resolves only in the Horizon home that made it. The key that this
//! computer uses for a machine is a local binding, [`super::MachineBindings`].
//!
//! Nothing here trusts a saved record: [`Machine::checked_terminal`] re-checks
//! the route and rebuilds the tmux command, and the caller re-derives the key
//! path from the key name, so a hand-edited record cannot turn a click into a
//! different program or an ssh option.

use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_terminal::{self, CheckedTerminal, TerminalRoute};
use serde::{Deserialize, Serialize};

use super::MachineError;
use crate::ssh::{MachineKeyRef, SshConnection};

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
    /// The tmux session that the terminal attaches to, when one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The provider line as the agent reported it, such as `Hetzner, hel1`. It
    /// is the agent's claim, not verified Horizon state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The repository that the machine is ready for, such as
    /// `example-org/shop`, when the agent set one. Horizon draws it and reads
    /// nothing else into it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub added_at_millis: u64,
    /// Whether the setup of the cards of the agents of this machine ran, so this
    /// window serves a card socket for each of its sessions. `machine_set` writes
    /// it, and the setup writes nothing on the machine without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cards: bool,
}

/// A machine whose name, route, session, repository and provider line passed
/// their checks: the only values that a record is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedMachine {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub session: Option<String>,
    pub provider: Option<String>,
    pub repository: Option<String>,
    pub cards: bool,
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
            session: self.session.clone(),
            provider: self.provider.clone(),
            repository: self.repository.clone(),
            added_at_millis,
            cards: self.cards,
        }
    }
}

impl Machine {
    /// Whether both records say the same thing about the machine, apart from the day
    /// that it was added: a server stamps a record with its own clock, and the stamp
    /// says nothing that a person can see or change.
    #[must_use]
    pub fn same_change(&self, other: &Self) -> bool {
        self.name == other.name
            && self.host == other.host
            && self.user == other.user
            && self.port == other.port
            && self.session == other.session
            && self.provider == other.provider
            && self.repository == other.repository
            && self.cards == other.cards
    }

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
        // The route passes the rule of an SSH terminal and the session passes the
        // tmux rule; the command itself is Horizon's own text, built by the one
        // builder that a placed workspace terminal uses, so a click on a machine
        // hides the tmux status line exactly as a terminal of a place does.
        let mut terminal = block_terminal::check(&route, None).map_err(MachineError::from)?;
        terminal.remote_command = session.as_deref().map(session_command);
        Ok(terminal)
    }

    /// The `ssh` command line of a click on this machine, with the key `key`: the
    /// command line that `connection_for` builds for the panel, with the bounds of
    /// a machine terminal.
    ///
    /// # Errors
    /// Refuses a record that no longer passes its rule.
    pub fn ssh_args(&self, key: &MachineKeyRef) -> Result<Vec<String>, MachineError> {
        Ok(
            SshConnection::for_agent_terminal_with_key(&self.checked_terminal()?, key)
                .as_machine_terminal()
                .to_command_args(),
        )
    }

    /// The title of a terminal that this machine opens.
    #[must_use]
    pub fn title(&self) -> String {
        self.name.clone()
    }
}

/// Check one machine record from an agent: its name, route, tmux session,
/// repository and provider line.
///
/// # Errors
/// Describes the first refused field with the code of [`MachineError::code`].
pub fn check(
    name: &str,
    route: &TerminalRoute,
    session: Option<&str>,
    provider: Option<&str>,
    repository: Option<&str>,
    cards: bool,
) -> Result<CheckedMachine, MachineError> {
    let name = machine_name(name)?;
    let terminal = block_terminal::check(route, None)?;
    let session = session.map(session_name).transpose()?;
    let provider = provider.map(block_machines::provider_line);
    let repository = repository.map(block_machines::repository_line);
    Ok(CheckedMachine {
        name: name.to_owned(),
        host: terminal.host,
        user: terminal.user,
        port: terminal.port,
        session,
        provider,
        repository,
        cards,
    })
}

/// Check one record that already exists, as a computer that did not make it reads
/// it: the name, the route, the tmux session and the provider line pass the same
/// rules as `machine_set`, and the returned record holds the checked values.
///
/// # Errors
/// Describes the first refused field with the code of [`MachineError::code`].
pub fn check_record(record: &Machine) -> Result<Machine, MachineError> {
    let route = TerminalRoute {
        host: record.host.clone(),
        user: record.user.clone(),
        port: record.port.map(i64::from),
    };
    let checked = check(
        &record.name,
        &route,
        record.session.as_deref(),
        record.provider.as_deref(),
        record.repository.as_deref(),
        record.cards,
    )?;
    Ok(checked.record(record.added_at_millis))
}

/// Check one machine name. It is the key name rule, so a record and the keys
/// that it may use have one name.
///
/// # Errors
/// Describes the rule with [`super::INVALID_MACHINE_NAME`].
pub fn machine_name(name: &str) -> Result<&str, MachineError> {
    block_machines::check_machine_name(name).map_err(|refusal| MachineError::InvalidName {
        message: refusal.message,
    })
}

/// Check one key name that a caller asks for a machine. The key stays optional:
/// a machine without a key connects with the person's own keys.
///
/// # Errors
/// Describes the rule with the key name code of the tool.
pub fn machine_key_name(key: &str) -> Result<String, MachineError> {
    horizon_browser_control::manifest::block_keys::check_name(key)
        .map(str::to_owned)
        .map_err(|refusal| MachineError::InvalidKey {
            message: refusal.message,
        })
}

/// The remote command that attaches to `session`, or makes it, and keeps the tmux
/// status line out of the last row of the panel.
///
/// One builder serves a terminal of a machine and a terminal of a workspace that
/// is placed on a machine, so no person sees the status line in one kind of
/// panel and not in the other. The `-u` flag keeps the text of the session as it
/// is: an `ssh` command of a machine often has no UTF-8 locale, and such a client
/// of `tmux` prints `_` in the place of every character outside ASCII.
#[must_use]
pub fn session_command(session: &str) -> String {
    format!("tmux -u new-session -A -s {session} \\; set-option status off")
}

/// Check one tmux session name.
///
/// # Errors
/// Describes the rule with [`super::INVALID_SESSION_NAME`].
pub fn session_name(session: &str) -> Result<String, MachineError> {
    block_machines::check_session_name(session)
        .map(str::to_owned)
        .map_err(|refusal| MachineError::InvalidSession {
            message: refusal.message,
        })
}
