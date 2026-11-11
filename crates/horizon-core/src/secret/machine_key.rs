//! One machine key as the server holds it: the private key, its public line and
//! the known-hosts lines of that key.
//!
//! The three files are one value, because a login needs all three to be the same
//! on every computer: the private key that `ssh -i` reads, the public line that
//! stands in `authorized_keys` of the machine, and the host key that first
//! contact pinned. A computer that receives this value writes the files where
//! [`crate::MachineKeys`] already reads them, with the mode of a private key.

use serde::{Deserialize, Serialize};

/// The files of one machine key, as one secret travels.
///
/// The [`std::fmt::Debug`] of this type names the lengths only: the text of a
/// private key is in no log line and in no error message.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MachineKeyFiles {
    /// The private half, which `ssh -i` reads.
    pub private_key: String,
    /// The public line, ready for `authorized_keys` of the machine.
    pub public_key: String,
    /// The known-hosts lines of this key alone. First contact with a machine
    /// pins its host key here, so a recycled address cannot collide with the
    /// person's own known hosts.
    #[serde(default)]
    pub known_hosts: String,
}

impl std::fmt::Debug for MachineKeyFiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MachineKeyFiles")
            .field("private_key", &format!("<{} bytes>", self.private_key.len()))
            .field("public_key", &self.public_key)
            .field("known_hosts", &format!("<{} bytes>", self.known_hosts.len()))
            .finish()
    }
}

impl MachineKeyFiles {
    /// Whether the value can be used as a key pair: a private key of this Horizon
    /// always holds more than one line, and the public half is one line of text.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.private_key.trim().is_empty() && !self.public_key.trim().is_empty()
    }
}
