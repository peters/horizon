//! The place of a workspace: this computer, or one machine of Horizon's machine
//! list.
//!
//! A workspace on a machine opens its new terminals on that machine. Horizon
//! builds the whole remote command here: a tmux session named from the workspace
//! and its panel, so a restart attaches to the same session (`new-session -A`)
//! and a close ends it. [`place_command`] is the only function that decides
//! which command a panel of a place runs, so another transport replaces its body
//! alone and no caller changes.
//!
//! Nothing here trusts the machine record: [`place_command`] re-checks its route
//! with the rule of an SSH terminal and takes the key by name from this Horizon
//! home, so a hand-edited registry cannot turn a new panel into another program
//! or an ssh option.

use std::{
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use horizon_browser_control::manifest::block_terminal::CheckedTerminal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::machines::{MACHINE_UNAVAILABLE, MachineError, Machines};
use crate::panel::{PanelKind, PanelOptions};
use crate::ssh::{MachineKeys, SshConnection};

mod reconnect;

pub use reconnect::{
    AfterConnectionLoss, ConnectionLoss, RECONNECT_RESET_AFTER, SSH_CONNECTION_FAILURE, after_connection_loss,
    attaches_a_tmux_session, reconnect_delay,
};

/// How many characters of a workspace local id name the tmux sessions of its
/// panels.
const SESSION_ID_CHARS: usize = 8;

/// The prefix of every tmux session that a place opens.
const SESSION_PREFIX: &str = "hz";

/// How long a session kill may run before Horizon stops it.
pub const SESSION_KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running session kill is polled for its end.
const KILL_POLL: Duration = Duration::from_millis(25);

/// Where a workspace lives: this computer, or one machine of the machine list.
///
/// A saved workspace without a place, and every workspace that Horizon does not
/// load from a snapshot, is on this computer. The place is saved as the machine
/// name, so a workspace on this computer writes no field at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum WorkspacePlace {
    /// This computer: a new terminal runs a shell here, as it always has.
    #[default]
    Local,
    /// One machine of [`crate::Machines`], by its name.
    Machine(String),
}

impl WorkspacePlace {
    /// The machine of this place, or `None` on this computer.
    #[must_use]
    pub fn machine(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Machine(name) => Some(name),
        }
    }

    /// Whether this place is this computer.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    /// Whether this place is the machine called `name`.
    #[must_use]
    pub fn is_machine(&self, name: &str) -> bool {
        self.machine() == Some(name)
    }
}

impl From<Option<String>> for WorkspacePlace {
    fn from(machine: Option<String>) -> Self {
        match machine.filter(|name| !name.trim().is_empty()) {
            Some(name) => Self::Machine(name),
            None => Self::Local,
        }
    }
}

impl From<WorkspacePlace> for Option<String> {
    fn from(place: WorkspacePlace) -> Self {
        place.machine().map(str::to_owned)
    }
}

impl Serialize for WorkspacePlace {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Option::<String>::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WorkspacePlace {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(Option::<String>::deserialize(deserializer)?))
    }
}

/// What [`place_command`] needs to know about the workspace that will hold the
/// new panel.
#[derive(Clone, Copy, Debug)]
pub struct PlaceWorkspace<'a> {
    /// The local id of the workspace. It names the tmux sessions of its panels,
    /// so two workspaces on one machine never share a session.
    pub local_id: &'a str,
    /// The tmux sessions that the panels of the workspace already hold.
    pub sessions: &'a [&'a str],
}

/// Why Horizon could not make a new panel of the place.
#[derive(Debug, thiserror::Error)]
pub enum PlaceError {
    /// The machine of the place is not in the machine list anymore.
    #[error(
        "the machine {machine} of this workspace is not in this Horizon's machine list anymore; put the workspace on another machine or on this computer"
    )]
    MachineUnavailable { machine: String },
    /// The record of the machine no longer passes the rule of an SSH terminal.
    #[error(transparent)]
    Machine(#[from] MachineError),
}

impl PlaceError {
    /// The refusal code: `machine_unavailable` for a place whose machine is
    /// gone, and the code of the record for a machine that fails its rule.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::MachineUnavailable { .. } => MACHINE_UNAVAILABLE,
            Self::Machine(error) => error.code(),
        }
    }
}

/// Whether a new panel of `kind` runs on the machine of its workspace: the two
/// panel kinds that hold a terminal a person types into. Agents, browsers,
/// devices, cards and editors stay on this computer in this version.
#[must_use]
pub const fn moves_to_the_machine(kind: PanelKind) -> bool {
    matches!(kind, PanelKind::Shell | PanelKind::Command)
}

/// Apply `place` to the options of one new panel of `workspace`.
///
/// A shell or command panel of a workspace on a machine becomes the SSH panel
/// that runs [`session_command`] there, with the machine key of the record when
/// it has one, and it saves the machine and its session name. Every other kind,
/// and every workspace on this computer, keeps the options of its caller.
///
/// # Errors
/// [`PlaceError::MachineUnavailable`] when the place names a machine that the
/// list no longer keeps, and the refusal of the record when its route no longer
/// passes the rule of an SSH terminal.
pub fn place_command(
    opts: &mut PanelOptions,
    place: &WorkspacePlace,
    machines: &Machines,
    keys: &MachineKeys,
    workspace: &PlaceWorkspace<'_>,
) -> Result<(), PlaceError> {
    let Some(name) = place.machine() else {
        return Ok(());
    };
    if !moves_to_the_machine(opts.kind) {
        return Ok(());
    }
    let Some(record) = machines.find(name) else {
        return Err(PlaceError::MachineUnavailable {
            machine: name.to_owned(),
        });
    };
    // The record is agent-written text: the route passes the rule of an SSH
    // terminal again here, exactly as a machine terminal does.
    let route = record.checked_terminal()?;
    let session = crate::machines::session_name(&next_session(workspace))?;
    let terminal = CheckedTerminal {
        host: route.host,
        user: route.user,
        port: route.port,
        remote_command: Some(session_command(&session, record.folder.as_deref())),
    };
    let connection = match machines.key(&record.name) {
        Some(key) => SshConnection::for_agent_terminal_with_key(&terminal, &keys.binding(key)),
        None => SshConnection::for_agent_terminal(&terminal),
    }
    .as_machine_terminal();
    opts.kind = PanelKind::Ssh;
    opts.command = None;
    opts.args.clear();
    opts.ssh_connection = Some(connection);
    opts.machine = Some(record.name.clone());
    opts.machine_session = Some(session);
    Ok(())
}

/// The tmux session of the next terminal of `workspace`: the smallest positive
/// number that no session of the workspace takes.
#[must_use]
fn next_session(workspace: &PlaceWorkspace<'_>) -> String {
    let prefix = session_prefix(workspace.local_id);
    let mut number = 1;
    while workspace
        .sessions
        .iter()
        .any(|session| session_number(session, &prefix) == Some(number))
    {
        number += 1;
    }
    format!("{prefix}-{number}")
}

/// The number that `session` adds to `prefix`, when it is one of its sessions.
fn session_number(session: &str, prefix: &str) -> Option<usize> {
    session.strip_prefix(prefix)?.strip_prefix('-')?.parse::<usize>().ok()
}

/// The prefix of the tmux sessions of `local_id`: `hz-` and eight lower-case
/// hexadecimal characters of the id, so a session is short enough for a person
/// to read in `tmux ls` and stable across restarts.
#[must_use]
pub fn session_prefix(local_id: &str) -> String {
    format!("{SESSION_PREFIX}-{}", id_hex(local_id))
}

/// The eight hexadecimal characters of `local_id`, in lower case. A local id
/// that holds fewer than eight, because it came from a hand-edited snapshot, is
/// hashed instead, so the prefix is still one word of the session rule.
fn id_hex(local_id: &str) -> String {
    let digits: String = local_id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|character| character.to_ascii_lowercase())
        .take(SESSION_ID_CHARS)
        .collect();
    if digits.len() == SESSION_ID_CHARS {
        return digits;
    }
    format!("{:08x}", fnv1a(local_id))
}

/// The 32 low bits of the FNV-1a hash of `text`: a value that does not change
/// between Horizon runs on one input.
fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The remote command of a session of a machine, which a terminal of a place and
/// a click on a machine row both run: attach to `session` (`-A`), or make it, and
/// keep the tmux status line out of the panel's last row.
pub use crate::machines::session_command;

/// The remote command that ends `session`.
#[must_use]
pub fn kill_command(session: &str) -> String {
    format!("tmux kill-session -t {session}")
}

/// The end of one place session, run best effort when a person closes its panel.
///
/// Quitting Horizon runs none of these: the sessions outlive the window, which
/// is the point of a workspace on a machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionKill {
    /// The tmux session to end.
    pub session: String,
    /// The program of the kill: `ssh`, as the panel's own command line runs it.
    pub program: String,
    /// The whole argv: the connection of the panel with [`kill_command`] in
    /// place of its session command.
    pub args: Vec<String>,
}

impl SessionKill {
    /// The kill of `session` over `connection`.
    #[must_use]
    pub fn over(connection: &SshConnection, session: &str) -> Self {
        let mut connection = connection.clone();
        connection.remote_command = Some(kill_command(session));
        Self {
            session: session.to_owned(),
            program: "ssh".to_owned(),
            args: connection.to_command_args(),
        }
    }

    /// Run the kill, best effort, with [`SESSION_KILL_TIMEOUT`]. A machine that
    /// does not answer is a line of the log, never an error dialog of the person
    /// who closed the panel. The caller runs this on a worker thread.
    pub fn run(&self) {
        self.run_within(SESSION_KILL_TIMEOUT);
    }

    /// [`Self::run`] with an explicit time limit, so a test does not wait for
    /// the production one.
    pub fn run_within(&self, limit: Duration) {
        match self.spawn_and_wait(limit) {
            Ok(status) if status.success() => {
                tracing::info!(session = %self.session, "ended the tmux session of a closed panel");
            }
            Ok(status) => {
                tracing::warn!(
                    session = %self.session,
                    code = status.code(),
                    "the tmux session of a closed panel may still run"
                );
            }
            Err(error) => {
                tracing::warn!(
                    session = %self.session,
                    %error,
                    "the tmux session of a closed panel may still run"
                );
            }
        }
    }

    /// Start the kill and wait for its end, at most `limit`.
    fn spawn_and_wait(&self, limit: Duration) -> std::io::Result<ExitStatus> {
        crate::ui_thread::blocking("the kill of a session runs and is awaited");
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("the kill of session {} did not end within {limit:?}", self.session),
                ));
            }
            std::thread::sleep(KILL_POLL);
        }
    }
}

#[cfg(test)]
mod tests;
