//! Named secrets: a value that a person stores once, and that every agent panel of
//! this computer gets as an environment variable.
//!
//! A tool that an agent starts often reads a key from the environment. A named
//! secret is that key, kept once for the person instead of exported again in the
//! shell of every computer. It has a name, which is an environment variable name,
//! and a value. The name lives in the settings and the value lives in the OS
//! credential store under the reference `named/<NAME>`, so no settings document
//! ever holds a value.
//!
//! Where a value goes, and where it never goes:
//!
//! - It becomes one environment variable of an agent panel that this process
//!   starts. A plain terminal panel, and a terminal on a machine over `ssh`, get
//!   none: a value never reaches a command line and never leaves this computer on
//!   a link that a person did not ask for.
//! - With a server set, it is one server secret of the id `named/<NAME>`, sealed at
//!   rest like every other secret, and it travels by id in the queue and never in a
//!   log line, a mirror or an error text.
//! - It is in no `tracing` line, no panel title, no saved session, no work ledger,
//!   no audit log and no crash text. A saved session holds the *kind* of a panel
//!   and its command, so a restored agent panel reads the value from the store
//!   again at its start instead of carrying it in a file.
//!
//! A panel keeps the environment of its start: a value that the person adds while a
//! panel runs is in the next panel that they start, which is what the Settings text
//! says.
//!
//! [`docs/review/trust-boundaries/named-secrets.md`](../../../docs/review/trust-boundaries/named-secrets.md)
//! states what is untrusted here, what this module guarantees and what it does not.

mod environment;
mod name;
mod store;
mod test_file;
mod workbench;

pub use environment::{apply, publish, published_names};
pub use name::{MAX_NAME_CHARS, NamedSecretError, SecretName, check_value, is_reserved};
pub use store::{NAMED_PREFIX, NAMED_SECRET_ORIGIN, NamedSecretChange, NamedSecretVault, locator};
pub use test_file::{TEST_FILE_ENV, TestFileNamedSecretStore};
pub use workbench::NamedSecretWorkbench;

use crate::remote_browser_credential::{KeyringCredentialStore, RemoteCredentialError, RemoteCredentialStore};

/// The most named secrets that one computer keeps.
pub const MAX_NAMED_SECRETS: usize = 64;

/// The most bytes of one value: 16 KiB, the bound of one server secret body.
pub const MAX_VALUE_BYTES: usize = 16 * 1024;

/// Whether this computer holds a value for one name, for the settings window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamedSecretState {
    /// This computer holds the value, and an agent panel started now gets it.
    Present,
    /// This computer holds no value, because the store refused it or because the
    /// value of the server has not arrived here yet.
    NoValue,
    /// The OS credential store is locked.
    Locked,
    /// This computer has no OS credential store.
    StoreUnavailable,
    /// A read of the store is in flight.
    Checking,
}

impl NamedSecretState {
    /// The line that the settings window shows under a name.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Present => "Stored on this computer",
            Self::NoValue => "no value on this computer",
            Self::Locked => "The credential store of this computer is locked",
            Self::StoreUnavailable => "This computer has no credential store",
            Self::Checking => "Checking this computer",
        }
    }
}

/// The store of the named secrets of this computer: the private file of an isolated
/// smoke fixture when that fixture names one, otherwise the OS credential store.
///
/// It opens a credential store, so it runs on a worker thread and never on the UI
/// thread.
#[must_use]
pub fn open_store() -> Option<Box<dyn RemoteCredentialStore + Send>> {
    match TestFileNamedSecretStore::from_environment() {
        Ok(Some(store)) => {
            tracing::info!(
                path = %store.path().display(),
                "the named secrets of this run are in the file of the smoke fixture, and not in an OS credential store"
            );
            return Some(Box::new(store));
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "the file of the named secrets of this run cannot be read");
            return None;
        }
    }
    match KeyringCredentialStore::open() {
        Ok(store) => Some(Box::new(store)),
        Err(error) => {
            tracing::info!(
                %error,
                "this computer has no OS credential store, so it keeps no named secret"
            );
            None
        }
    }
}

/// Why a name, a value or the store of this computer refused a named secret, in the
/// shape that a store operation answers.
pub(crate) fn map_store_error(name: &SecretName, error: &RemoteCredentialError) -> NamedSecretError {
    match error {
        RemoteCredentialError::Locked => NamedSecretError::Locked,
        RemoteCredentialError::StoreUnavailable => NamedSecretError::StoreUnavailable,
        RemoteCredentialError::InvalidValue | RemoteCredentialError::NotHeaderSafe => {
            NamedSecretError::InvalidValue { name: name.to_string() }
        }
        RemoteCredentialError::Platform { kind } => NamedSecretError::Platform { kind },
        _ => NamedSecretError::Platform { kind: "refused" },
    }
}

#[cfg(test)]
mod tests;
