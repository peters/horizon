//! The command that opens the agent of a machine in a terminal panel, and whether a
//! session of a machine can be opened at all.
//!
//! The command is the one that a terminal of a machine builds
//! ([`crate::machines::session_command`]), and the session name passes the rule of this
//! build first, because the name is one word of that command: a `tmux` session that a
//! person made with a space is listed and cannot be opened.

use crate::machines::{MachineError, session_command, session_name};

use super::state::MachineAgent;

impl MachineAgent {
    /// The remote command that opens this agent in a terminal panel of its machine.
    ///
    /// # Errors
    /// [`MachineError::InvalidSession`] for a session name outside the rule of this
    /// build, which a person may have made by hand in `tmux`.
    pub fn open_command(&self) -> Result<String, MachineError> {
        Ok(session_command(&session_name(&self.session)?))
    }

    /// Whether this build can open this agent: a session whose name holds a space or
    /// another character outside the rule is one that Horizon does not guess.
    #[must_use]
    pub fn can_open(&self) -> bool {
        session_name(&self.session).is_ok()
    }
}
