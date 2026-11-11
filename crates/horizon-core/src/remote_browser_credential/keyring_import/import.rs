//! One import of the keyring: the work that a click starts.
//!
//! The work of a click runs on a worker thread of the Settings tab, so the UI thread
//! never reads a keyring, never starts a program and never waits for one.

use super::outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport};
use super::plan::KeyringImportPlan;
use super::program::ProgramEnvironment;
use super::readers::{self, Bound};
use super::run;
use crate::remote_browser_credential::SharedStore;

/// The work of one click: read every entry that the system can name, then write the
/// rows that these settings name.
///
/// # Errors
/// The typed refusal of the first rule that stops the import: a keyring that this
/// computer cannot read, an entry that does not parse, a reference that breaks the
/// reference rule, a value that the store refuses, or a secrets file that cannot be
/// read or written.
pub fn import_from_keyring(
    plan: &KeyringImportPlan,
    environment: &ProgramEnvironment,
    bound: Bound,
    store: &SharedStore,
) -> Result<KeyringImportOutcome, KeyringImportError> {
    if plan.is_empty() {
        return Err(KeyringImportError::NothingToDo);
    }
    let entries = readers::entries(plan, environment, bound)?;
    if entries.is_empty() {
        return Ok(KeyringImportOutcome::new(KeyringImportReport::default(), Vec::new()));
    }
    // The read of the keyring waits on a program for up to the bound, so this lock
    // is taken after it and held for the writes alone.
    let mut held = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    run::import(entries, plan, held.as_mut())
}
