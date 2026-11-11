//! Reading the values of an older Horizon from the keyring of this system.
//!
//! Three programs read them, and none of them is a crate: Linux reads the Secret
//! Service with `secret-tool` from `libsecret-tools`, or with the `gi` binding of
//! libsecret through `python3` when `secret-tool` is not installed; macOS reads the
//! Keychain with `security`. Every reading is untrusted text that names an account
//! and carries a value, so a reading that does not match the format of its program
//! exactly is [`KeyringImportError::Unreadable`], and no value leaves this module
//! except as a [`KeyringEntry`], whose [`Debug`](std::fmt::Debug) is redacted.

pub(crate) mod libsecret;
pub(crate) mod secret_tool;
pub(crate) mod security;

pub(crate) mod platform;

use std::time::Duration;

use zeroize::Zeroizing;

use super::KeyringImportError;
use super::program::{ProgramFailure, ProgramOutput};

pub(crate) use platform::entries;

/// The service name that an older Horizon gave every item of a remote browser
/// provider, as `KEYRING_SERVICE` of that version wrote it.
pub(crate) const SERVICE: &str = "horizon-remote-browser";

/// How long the programs of one import may run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bound {
    /// One process answers every entry, so the bound covers the whole import.
    WholeImport(Duration),
    /// The system may ask the person for each entry, so the bound covers one
    /// entry and not the import beside it.
    EachEntry(Duration),
}

impl Bound {
    /// The bound of this import, as the message of a passed bound names it.
    #[must_use]
    pub fn seconds(self) -> u64 {
        match self {
            Self::WholeImport(bound) | Self::EachEntry(bound) => bound.as_secs(),
        }
    }

    /// The bound of one child of this import.
    fn program(self) -> Duration {
        match self {
            Self::WholeImport(bound) | Self::EachEntry(bound) => bound,
        }
    }
}

/// One entry of the keyring of an older Horizon.
pub struct KeyringEntry {
    /// The account that the item names, which an older Horizon wrote as
    /// `<origin>|<slot>`.
    pub user: String,
    /// The value of the item, wiped when it goes.
    pub value: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for KeyringEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KeyringEntry")
            .field("user", &self.user)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Turn the failure of one child into the typed refusal of the import.
pub(crate) fn failure(program: &'static str, failure: ProgramFailure, bound: Bound) -> KeyringImportError {
    match failure {
        ProgramFailure::TimedOut => KeyringImportError::TimedOut {
            seconds: bound.seconds(),
        },
        ProgramFailure::Failed => KeyringImportError::NotRunnable { program },
    }
}

/// The typed refusal of a program that ended with a failure.
pub(crate) fn ended(program: &'static str, output: &ProgramOutput) -> KeyringImportError {
    match output.code() {
        Some(code) => KeyringImportError::ProgramFailed { program, code },
        None => KeyringImportError::ProgramSignalled { program },
    }
}

/// The lines of one stream, each without its newline.
///
/// Every program of the import ends a line with a newline, so a stream that does
/// not end with one is not the answer of such a program: a value that held the
/// last newline of a stream would look like an answer and is refused instead.
pub(crate) fn lines(bytes: &[u8]) -> Result<Vec<&[u8]>, KeyringImportError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = bytes.strip_suffix(b"\n") else {
        return Err(KeyringImportError::Unreadable);
    };
    Ok(body.split(|byte| *byte == b'\n').collect())
}

/// The text of one field of a line, checked before it is read.
pub(crate) fn text(field: &[u8]) -> Result<&str, KeyringImportError> {
    std::str::from_utf8(field).map_err(|_| KeyringImportError::Unreadable)
}

/// Whether an output of one program mentions `needle`.
///
/// One program prints the documented name of a D-Bus error and another the message
/// of a missing Python module. Those names choose the sentence of a refusal; no
/// other text of a program is read, and none of it is shown.
pub(crate) fn mentions(bytes: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
}

/// The value of one line that a program wrote as `prefix<value>`, or `None` when
/// the line names another field.
pub(crate) fn field<'a>(line: &'a [u8], prefix: &str) -> Option<&'a [u8]> {
    line.strip_prefix(prefix.as_bytes())
}
