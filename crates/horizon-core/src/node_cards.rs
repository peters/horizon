//! The cards of an agent that runs on a machine of the machine list.
//!
//! What both sides of one card link agree on: the socket path of a session on the
//! machine and here, and the identity that this computer mints for it. The relay
//! command, the MCP entry and the words of the agent on the machine are the text
//! of the machines skill, which the setup of a person runs there; no code of
//! Horizon writes them. The design is `docs/architecture/machine-agent-cards.md`.

mod paths;

pub use paths::{actor, actor_parts, local_dir, local_socket, remote_socket};

#[cfg(test)]
mod tests;

use horizon_browser_control::manifest::block_machines::{self, MachineRefusal};

/// Refusal code of a socket path that no Unix listener of this build takes.
const SOCKET_PATH_TOO_LONG: &str = "socket_path_too_long";

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
    if path.len() <= paths::MAX_SOCKET_PATH_BYTES {
        return Ok(());
    }
    Err(MachineRefusal {
        code: SOCKET_PATH_TOO_LONG,
        message: format!(
            "the socket path is {} bytes, above the {} that a Unix socket takes; start Horizon from a shorter temporary folder",
            path.len(),
            paths::MAX_SOCKET_PATH_BYTES
        ),
    })
}
