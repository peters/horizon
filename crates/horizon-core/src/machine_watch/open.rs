//! The command that opens the agent of a machine in a terminal panel, and whether a
//! session of a machine can be opened at all.
//!
//! The command is the one that a terminal of a machine and a terminal of a placed
//! workspace already build ([`crate::machines::session_command`]): it attaches the
//! `tmux` session, or makes it, and it keeps the status line of `tmux` out of the
//! last row of the panel. The session name passes the rule of this build before it
//! reaches that command, because the name is one word of it: a `tmux` session that a
//! person made with another character, such as a space, is listed by the watch and
//! cannot be opened, and its row says so.
//!
//! The next round builds the panel; this module builds the text alone.

use crate::machines::{MachineError, session_command, session_name};

use super::state::MachineAgent;

/// The remote command that opens `agent` in a terminal panel of its machine.
///
/// # Errors
/// [`MachineError::InvalidSession`] when the session name of the agent is not one
/// that this build passes to `tmux`.
pub fn open_command(agent: &MachineAgent) -> Result<String, MachineError> {
    Ok(session_command(&session_name(&agent.session)?))
}

/// Whether this build can open the session of `agent`.
///
/// A `tmux` session whose name holds a space or another character outside the rule
/// of this build is one that the person made by hand, and Horizon does not guess
/// how to pass it to a command line.
#[must_use]
pub fn can_open(agent: &MachineAgent) -> bool {
    session_name(&agent.session).is_ok()
}

impl MachineAgent {
    /// The remote command that opens this agent, or why its session name is not one
    /// that this build opens.
    ///
    /// # Errors
    /// [`MachineError::InvalidSession`] for a session name outside the rule.
    pub fn open_command(&self) -> Result<String, MachineError> {
        open_command(self)
    }

    /// Whether this build can open this agent.
    #[must_use]
    pub fn can_open(&self) -> bool {
        can_open(self)
    }
}
