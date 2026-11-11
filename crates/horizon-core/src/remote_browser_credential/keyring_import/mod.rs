//! A one-time import of the values that an older Horizon kept in the keyring of
//! this system.
//!
//! An earlier version of Horizon kept a provider value in the Secret Service of
//! Linux or in the Keychain of macOS. This version keeps every value in the one
//! secrets file of the Horizon home, so a person who used the older version would
//! type every value again. This module reads those values once, when the person
//! asks for it, and writes the rows that this version reads.
//!
//! **The keyring crates do not come back.** No crate of this workspace opens the
//! keyring: three programs of the system answer instead, and
//! [`docs/platform-support.md`](../../../../../docs/platform-support.md) names them.
//! Each program runs as a child of this process with a cleared environment, a bound,
//! and a pipe that no value leaves except into a [`zeroize`]d buffer. The values
//! never reach an argument, an environment variable, a log line, an error text or a
//! temporary file, and neither [`KeyringImportReport`] nor [`KeyringImportError`] can
//! hold one.
//!
//! **Nothing runs by itself.** A start of Horizon reads no keyring. The Settings tab
//! "Remote" offers one line and one button while a reference of this computer has no
//! value, and a click starts one import on a worker thread.
//!
//! **The entry stays.** An import copies the value into the secrets file and leaves
//! the keyring item as it is, so a person who goes back to the older version still
//! has its value. A reference that already has a row is never written over.
//!
//! [`docs/review/trust-boundaries/named-secrets.md`](../../../../../docs/review/trust-boundaries/named-secrets.md)
//! states what is untrusted here and what this import does not guarantee.

mod click;
mod import;
mod outcome;
mod plan;
mod program;
mod readers;
mod run;

pub use click::{ImportLine, KeyringImport};
pub use import::import_from_keyring;
pub use outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport};
pub use plan::KeyringImportPlan;
pub use program::ProgramEnvironment;
pub use readers::{Bound, Platform};

#[cfg(test)]
mod tests;
