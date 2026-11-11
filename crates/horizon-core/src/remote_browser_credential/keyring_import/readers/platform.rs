//! Which program this system reads a keyring with, how one import asks it, and the
//! words of a refusal of this system.
//!
//! Every reader in this folder is compiled on both systems, so the format of each
//! program is tested on the computer that runs the tests. Only the choice belongs to
//! the system: macOS reads the Keychain with `security`, and every other system reads
//! the Secret Service with `secret-tool` or, when that program is not installed, with
//! the `gi` binding of libsecret through `python3`.
//!
//! [`Platform`] is a value and not a condition of the build, so a test reads the
//! entries and the sentences of either system on the computer that runs the tests.

use std::time::Duration;

use super::super::KeyringImportError;
use super::super::plan::KeyringImportPlan;
use super::super::program::ProgramEnvironment;
use super::{Bound, KeyringEntry, libsecret, secret_tool, security};

/// The keyring of one system, as the programs that read it and the words of its
/// refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    /// The Secret Service of Linux. One process answers every entry.
    SecretService,
    /// The Keychain of macOS. One process answers one account, and the system may
    /// ask the person for each of them.
    Keychain,
}

impl Platform {
    /// The keyring of the system that runs this build.
    #[must_use]
    pub const fn of_this_system() -> Self {
        if cfg!(target_os = "macos") {
            Self::Keychain
        } else {
            Self::SecretService
        }
    }

    /// The bound of one import of this keyring: the Secret Service answers every
    /// entry within one process, and the Keychain may ask the person once for each
    /// entry.
    #[must_use]
    pub const fn bound(self) -> Bound {
        match self {
            Self::SecretService => Bound::WholeImport(Duration::from_secs(20)),
            Self::Keychain => Bound::EachEntry(Duration::from_secs(60)),
        }
    }

    /// The sentence of a refusal when the program of this keyring is missing.
    ///
    /// The two systems need different sentences: a Mac has no package
    /// `libsecret-tools`, and `security` is a part of macOS.
    #[must_use]
    pub const fn missing_program(self) -> &'static str {
        match self {
            Self::SecretService => {
                "Install the package `libsecret-tools` to read the values that an older version stored in the keyring."
            }
            Self::Keychain => "The program `security` of macOS was not found on this computer.",
        }
    }

    /// The entries of the keyring that `plan` can use, read with the programs of this
    /// keyring.
    ///
    /// Both readings are compiled on both systems, because a test of either reading
    /// runs on the computer that runs the tests. The choice is a value, so a test
    /// reads the entries of the other system too.
    pub(crate) fn entries(
        self,
        plan: &KeyringImportPlan,
        environment: &ProgramEnvironment,
        bound: Bound,
    ) -> Result<Vec<KeyringEntry>, KeyringImportError> {
        match self {
            Self::Keychain => each(environment, bound, &plan.users()),
            Self::SecretService => secret_service(environment, bound),
        }
    }
}

/// The Secret Service answers every entry at once, so one process is the whole
/// import. A closed item asks the person to unlock it, which the bound covers.
pub(crate) fn secret_service(
    environment: &ProgramEnvironment,
    bound: Bound,
) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    match environment.find(secret_tool::PROGRAM) {
        Some(program) => secret_tool::read(&program, environment, bound),
        None => libsecret::read(environment, bound),
    }
}

/// The Keychain answers one account at a time, and the system may ask the person
/// once for each answer. An account that the keychain does not hold says nothing.
pub(crate) fn each(
    environment: &ProgramEnvironment,
    bound: Bound,
    users: &[&str],
) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    let mut entries = Vec::new();
    for user in users {
        let Some(value) = security::read(environment, user, bound)? else {
            continue;
        };
        entries.push(KeyringEntry {
            user: (*user).to_string(),
            value,
        });
    }
    Ok(entries)
}
