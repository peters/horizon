//! One secret that Horizon keeps, and the place on this computer where it lives.
//!
//! A secret is one value that more than one computer of a person holds: the pair
//! of a machine key with its known-hosts lines, or one credential value of a
//! provider. The same value has one id on the server, so a second computer that
//! follows the server writes it where the code of that computer already reads it
//! and works without the server afterwards.
//!
//! An id names the kind of value and the name that Horizon uses for it:
//! `machine-key/<name>` for one pair of [`crate::MachineKeys`] and
//! `credential/<reference>` for the value under one credential reference of a
//! remote browser provider. The id is checked here, before a value reaches any
//! file, so an id from a server is never a path and never a command.
//!
//! The body of a secret is opaque text to the server. This crate decides what the
//! text holds: JSON of [`MachineKeyFiles`] for a machine key, the value itself
//! for a credential.

use std::fmt;

use zeroize::Zeroizing;

mod credential;
mod machine_key;
mod vault;

pub use credential::{CredentialVault, os_locators, read_secret_value};
pub use machine_key::MachineKeyFiles;
pub use vault::SecretVault;

/// The most secrets that a server keeps.
pub const MAX_SECRETS: u64 = 256;

/// The most bytes of one secret body.
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// The prefix of the id of a machine key.
pub const MACHINE_KEY_PREFIX: &str = "machine-key/";

/// The prefix of the id of a credential value.
pub const CREDENTIAL_PREFIX: &str = "credential/";

/// The most characters of an id: the prefix and the longest name behind it.
pub const MAX_SECRET_ID_CHARS: usize = 80;

/// The longest reference of a credential, as the remote browser configuration
/// allows it.
const MAX_REFERENCE_CHARS: usize = 64;

/// The longest name of a machine key, as `machine_key` allows it.
const MAX_KEY_NAME_CHARS: usize = 40;

/// What one secret holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretKind {
    /// The private key, the public line and the known-hosts lines of one machine
    /// key of this Horizon home.
    MachineKey,
    /// The value under one credential reference of a remote browser provider, in
    /// the OS credential store of this computer.
    Credential,
}

impl SecretKind {
    /// The stable code of this kind, for a log line and a test.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::MachineKey => "machine-key",
            Self::Credential => "credential",
        }
    }
}

/// The id of one secret, in the form that the server holds it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretId {
    text: String,
    kind: SecretKind,
}

impl SecretId {
    /// The id of the machine key of `name`.
    ///
    /// # Errors
    /// Refuses a name that is not one key name.
    pub fn machine_key(name: &str) -> Result<Self, SecretError> {
        if name.is_empty()
            || name.chars().count() > MAX_KEY_NAME_CHARS
            || name.starts_with('-')
            || name.ends_with('-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(SecretError::InvalidId {
                message: format!(
                    "The machine key {name:?} is not 1 to {MAX_KEY_NAME_CHARS} characters of a-z, 0-9 and -, with no - at either end"
                ),
            });
        }
        Ok(Self {
            text: format!("{MACHINE_KEY_PREFIX}{name}"),
            kind: SecretKind::MachineKey,
        })
    }

    /// The id of the credential value under `reference`.
    ///
    /// # Errors
    /// Refuses a reference that is not one credential reference.
    pub fn credential(reference: &str) -> Result<Self, SecretError> {
        if reference.is_empty()
            || reference.chars().count() > MAX_REFERENCE_CHARS
            || !reference
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(SecretError::InvalidId {
                message: format!(
                    "The credential reference {reference:?} is not 1 to {MAX_REFERENCE_CHARS} characters of letters, digits, '.', '_' and '-'"
                ),
            });
        }
        Ok(Self {
            text: format!("{CREDENTIAL_PREFIX}{reference}"),
            kind: SecretKind::Credential,
        })
    }

    /// The id that `text` names.
    ///
    /// # Errors
    /// Refuses a text that is not one id of this Horizon, so an id that a server
    /// sends is never a path, a command or a longer name than a name.
    pub fn parse(text: &str) -> Result<Self, SecretError> {
        let (prefix, rest) = text.split_once('/').ok_or_else(|| Self::invalid(text))?;
        match prefix {
            "machine-key" => Self::machine_key(rest).map_err(|_| Self::invalid(text)),
            "credential" => Self::credential(rest).map_err(|_| Self::invalid(text)),
            _ => Err(Self::invalid(text)),
        }
    }

    fn invalid(text: &str) -> SecretError {
        let quoted: String = text.chars().take(MAX_SECRET_ID_CHARS).collect();
        SecretError::InvalidId {
            message: format!(
                "The secret id {quoted:?} is not one id of this Horizon: machine-key/<name> or credential/<reference>"
            ),
        }
    }

    /// The kind of value that this id names.
    #[must_use]
    pub fn kind(&self) -> SecretKind {
        self.kind
    }

    /// The name behind the prefix: the name of a machine key, or the reference of
    /// a credential.
    #[must_use]
    pub fn name(&self) -> &str {
        self.text.split_once('/').map_or(self.text.as_str(), |(_, rest)| rest)
    }

    /// The id as the server holds it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for SecretId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

/// Why this computer could not read, keep or forget one secret.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The id is not one id of this Horizon.
    #[error("{message}")]
    InvalidId { message: String },
    /// The value cannot be used on this computer as it is.
    #[error("{message}")]
    Unreadable { message: String },
    /// This computer has no place for this kind of value.
    #[error("{reason}")]
    Unsupported { reason: String },
    /// A file or a credential store of this computer failed.
    #[error("{reason}")]
    Unavailable { reason: String },
}

impl SecretError {
    /// The stable code of this error, for a log line and a notice.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidId { .. } => "invalid_secret_id",
            Self::Unreadable { .. } => "unreadable_secret",
            Self::Unsupported { .. } => "unsupported_secret",
            Self::Unavailable { .. } => "secret_unavailable",
        }
    }
}

/// The place where this computer keeps its secrets, one per kind.
///
/// The two kinds live where the code of this computer already reads them, so a
/// value that arrives from the server is used without another change: a machine
/// key under `machine-keys/<name>` of the Horizon home, a credential value in the
/// OS credential store under the reference of its provider.
pub trait SecretTarget: Send + Sync + fmt::Debug {
    /// The value of `id`, or `None` when this computer does not hold it.
    ///
    /// # Errors
    /// Returns why the value cannot be read.
    fn read(&self, id: &SecretId) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;

    /// Writes the value of `id`, replacing a value that is there.
    ///
    /// # Errors
    /// Returns why the value cannot be written.
    fn write(&self, id: &SecretId, body: &[u8]) -> Result<(), SecretError>;

    /// Forgets the value of `id`. A value that is not there is already forgotten.
    ///
    /// # Errors
    /// Returns why the value cannot be removed.
    fn remove(&self, id: &SecretId) -> Result<(), SecretError>;

    /// Keeps this local value beside the one that the server decided, and answers
    /// one line that says where it went, for the person.
    ///
    /// # Errors
    /// Returns why the value cannot be kept.
    fn keep_local(&self, id: &SecretId, body: &[u8], at_millis: u64) -> Result<String, SecretError>;

    /// Takes the value that the server decided, and keeps a local value that
    /// differs when `keep_local` says that this computer never published it.
    /// Answers the line for the person, when it kept one.
    ///
    /// # Errors
    /// Returns why the local value cannot be read, kept or replaced.
    fn take_over(
        &self,
        id: &SecretId,
        body: &[u8],
        keep_local: bool,
        at_millis: u64,
    ) -> Result<Option<String>, SecretError> {
        let notice = if keep_local {
            match self.read(id)? {
                Some(local) if local.as_slice() != body => Some(self.keep_local(id, &local, at_millis)?),
                _ => None,
            }
        } else {
            None
        };
        self.write(id, body)?;
        Ok(notice)
    }

    /// The secrets of `kind` that this computer holds now, as ids. It is what a
    /// computer whose server has no row for a value sends again.
    fn local_ids(&self, kind: SecretKind) -> Vec<String>;
}

#[cfg(test)]
mod tests;
