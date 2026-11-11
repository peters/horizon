//! The name of one named secret, and the rule of its value.
//!
//! A name is the name of an environment variable, because that is what the value
//! becomes in an agent panel. The names that Horizon and the system own are
//! refused: a named secret must never decide how a program finds its home, its
//! shell, its display or its agent, and it must never overwrite a variable that
//! Horizon sets for its own tools.

use std::fmt;

use super::MAX_VALUE_BYTES;

/// The longest name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// The variables that Horizon, an agent or the system own. A named secret with
/// one of these names is refused, so no value replaces the home, the shell, the
/// display or the agent of a program that Horizon starts.
const RESERVED_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TERM",
    "LANG",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "TMPDIR",
];

/// The prefixes of the names that Horizon, an agent or the system own.
const RESERVED_PREFIXES: &[&str] = &[
    "LD_",
    "DYLD_",
    "SSH_",
    "HORIZON",
    "CLAUDE",
    "CODEX",
    "ANTHROPIC",
    "OPENAI",
    "XDG_",
    "DBUS_",
];

/// The name of one named secret: an environment variable name that is not a name
/// that Horizon or the system owns.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

impl SecretName {
    /// `text` as the name of one named secret.
    ///
    /// # Errors
    /// A name that is not 1 to 64 characters of `A-Z`, `0-9` and `_`, a name
    /// that does not start with `A-Z`, and a name that Horizon or the system
    /// own.
    pub fn parse(text: &str) -> Result<Self, NamedSecretError> {
        if text.is_empty()
            || text.chars().count() > MAX_NAME_CHARS
            || !text.starts_with(|character: char| character.is_ascii_uppercase())
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(NamedSecretError::InvalidName { name: text.to_owned() });
        }
        if is_reserved(text) {
            return Err(NamedSecretError::ReservedName { name: text.to_owned() });
        }
        Ok(Self(text.to_owned()))
    }

    /// The name, as an environment variable name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for SecretName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SecretName").field(&self.0).finish()
    }
}

/// Whether Horizon or the system owns `name`.
#[must_use]
pub fn is_reserved(name: &str) -> bool {
    RESERVED_NAMES.contains(&name) || RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Checks one value that a person typed, or that a server sent, before it is kept
/// or given to a panel.
///
/// # Errors
/// A value that is empty, larger than [`MAX_VALUE_BYTES`], holds a NUL byte or a
/// line break, or is not text.
pub fn check_value(name: &SecretName, bytes: &[u8]) -> Result<(), NamedSecretError> {
    if bytes.is_empty() || bytes.len() > MAX_VALUE_BYTES {
        return Err(NamedSecretError::InvalidValue { name: name.to_string() });
    }
    if bytes.iter().any(|byte| matches!(byte, b'\0' | b'\n' | b'\r')) || std::str::from_utf8(bytes).is_err() {
        return Err(NamedSecretError::ValueNotText { name: name.to_string() });
    }
    Ok(())
}

/// Why a name, a value or the store of this computer refused a named secret.
///
/// The text never holds a value: a refusal names the secret and never quotes it.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NamedSecretError {
    #[error("The name {name:?} is not 1 to {MAX_NAME_CHARS} characters of A-Z, 0-9 and _, and it starts with A-Z")]
    InvalidName { name: String },
    #[error("The name {name} belongs to Horizon or to the system, so a named secret does not use it")]
    ReservedName { name: String },
    #[error("The value of {name} is empty or larger than {MAX_VALUE_BYTES} bytes")]
    InvalidValue { name: String },
    #[error("The value of {name} holds a NUL byte or a line break, or it is not text")]
    ValueNotText { name: String },
    #[error("This computer has no credential store, so it keeps no named secret")]
    StoreUnavailable,
    #[error("The credential store of this computer is locked")]
    Locked,
    #[error("The credential store of this computer failed: {kind}")]
    Platform { kind: &'static str },
}

impl NamedSecretError {
    /// The stable code of this error, for a log line and a notice.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidName { .. } => "invalid_secret_name",
            Self::ReservedName { .. } => "reserved_secret_name",
            Self::InvalidValue { .. } => "invalid_secret_value",
            Self::ValueNotText { .. } => "secret_value_not_text",
            Self::StoreUnavailable => "secret_store_unavailable",
            Self::Locked => "secret_store_locked",
            Self::Platform { .. } => "secret_store_failed",
        }
    }
}
