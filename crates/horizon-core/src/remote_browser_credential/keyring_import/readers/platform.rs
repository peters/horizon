//! Which program this system reads a keyring with, and how one import asks it.
//!
//! Every reader in this folder is compiled on both systems, so the format of each
//! program is tested on the computer that runs the tests. Only the choice below
//! belongs to the system: macOS reads the Keychain with `security`, and every other
//! system reads the Secret Service with `secret-tool` or, when that program is not
//! installed, with the `gi` binding of libsecret through `python3`.

use super::super::KeyringImportError;
use super::super::plan::KeyringImportPlan;
use super::super::program::ProgramEnvironment;
use super::{Bound, KeyringEntry, libsecret, secret_tool, security};

/// The entries of the keyring that `plan` can use, read with the program of this
/// system.
///
/// Both readings are compiled on both systems, because a test of either reading
/// runs on the computer that runs the tests. The condition is a constant of the
/// build, so a build of one system runs one reading and holds no dead code.
pub(crate) fn entries(
    plan: &KeyringImportPlan,
    environment: &ProgramEnvironment,
    bound: Bound,
) -> Result<Vec<KeyringEntry>, KeyringImportError> {
    if cfg!(target_os = "macos") {
        each(environment, bound, &plan.users())
    } else {
        secret_service(environment, bound)
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
