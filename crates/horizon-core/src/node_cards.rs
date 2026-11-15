//! The cards of an agent that runs on a machine of the machine list.
//!
//! An agent program runs in a `tmux` session of a machine. Horizon gives that
//! session one socket on the machine, and the `ssh` child of this computer
//! carries it to one Unix socket here. The agent's MCP entry is one relay
//! command: it reads the `tmux` session that the agent runs in, finds that
//! session's socket and copies the byte stream of the agent through it.
//!
//! This module holds what both sides of that link agree on: the socket path of
//! one session, the identity that Horizon mints for it, the relay command and the
//! entry texts of the agent programs. The machine states no identity: this
//! computer builds it from the two names after [`block_machines`] checked both.
//! The boundary is `docs/review/trust-boundaries/machine-agent-cards.md`.

mod entry;
mod paths;
mod relay;

pub use entry::{CardAgent, entry_file, entry_json, entry_toml};
pub use paths::{
    ACTOR_PREFIX, ENTRY_NAME, MAX_SOCKET_PATH_BYTES, REMOTE_DIR, actor, actor_parts, local_dir, local_socket,
    remote_socket,
};
pub use relay::{PROBE_COMMAND, PYTHON_RELAY, RelayCommand, RelayProgram, probe_outcome, relay_command};

#[cfg(test)]
mod tests;

use horizon_browser_control::manifest::block_machines::{self, MachineRefusal};

/// Refusal code of a socket path that no Unix listener of this build takes.
pub const SOCKET_PATH_TOO_LONG: &str = "socket_path_too_long";

/// Refusal code of a machine that runs no program to relay the stream of an
/// agent.
pub const RELAY_MISSING: &str = "relay_missing";

/// The two names of one session of a machine, which every path and the actor of
/// the session are built from.
pub(crate) fn check_pair<'a>(machine: &'a str, session: &'a str) -> Result<(&'a str, &'a str), MachineRefusal> {
    Ok((
        block_machines::check_machine_name(machine)?,
        block_machines::check_session_name(session)?,
    ))
}

/// Refuse a Unix socket path that a listener of this build would not take.
pub(crate) fn check_socket_path(path: &str) -> Result<(), MachineRefusal> {
    if path.len() <= MAX_SOCKET_PATH_BYTES {
        return Ok(());
    }
    Err(MachineRefusal {
        code: SOCKET_PATH_TOO_LONG,
        message: format!(
            "the socket path is {} bytes, above the {MAX_SOCKET_PATH_BYTES} that a Unix socket takes; start Horizon from a shorter temporary folder",
            path.len()
        ),
    })
}
