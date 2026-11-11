//! What one import does with the entries that a program of this system read.
//!
//! Three rules of the secrets file apply here, and they apply in this order:
//!
//! 1. A reference passes the reference rule of [`SecretId`] before a row is named.
//!    The reference comes from these settings and never from the keyring, and it is
//!    checked again so an edited settings file cannot name a row that is no row.
//! 2. A reference that already has a row keeps its value. An import never writes
//!    over a value that this computer holds, so a person who typed a value again
//!    after the upgrade does not lose it.
//! 3. A row is written through the store that a typed value uses, so the file
//!    checks the value with [`validate_secret`](super::super::validate_secret) and
//!    the link to a server sends it like any other value.
//!
//! The keyring entry stays as it is: a person who goes back to the older version
//! still finds its value.

use crate::SecretId;

use super::super::{CredentialValueChange, RemoteCredentialError, RemoteCredentialStore};
use super::outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport};
use super::plan::KeyringImportPlan;
use super::readers::KeyringEntry;

/// Write the row of every reference that `plan` names from the entries of the
/// keyring, and answer the counts of the line and the changes of the link.
pub(super) fn import(
    entries: Vec<KeyringEntry>,
    plan: &KeyringImportPlan,
    store: &mut dyn RemoteCredentialStore,
) -> Result<KeyringImportOutcome, KeyringImportError> {
    check_references(plan)?;
    let mut report = KeyringImportReport::default();
    let mut changes = Vec::new();
    for entry in entries {
        let references = plan.references_of(&entry.user);
        if references.is_empty() {
            report.skipped();
            continue;
        }
        for planned in references {
            if store.contains(&planned.locator)? {
                report.kept();
                continue;
            }
            store.put(&planned.locator, &entry.value).map_err(refusal)?;
            report.written();
            changes.push(CredentialValueChange {
                reference: planned.locator.reference.clone(),
                stored: true,
            });
        }
    }
    Ok(KeyringImportOutcome::new(report, changes))
}

/// Every reference of the plan must pass the reference rule before any row is
/// written, so a settings file that breaks the rule writes no row at all.
fn check_references(plan: &KeyringImportPlan) -> Result<(), KeyringImportError> {
    for locator in plan.locators() {
        let reference = locator.reference.as_str();
        if SecretId::credential(reference).is_err() {
            return Err(KeyringImportError::RefusedReference {
                reference: reference.to_string(),
            });
        }
    }
    Ok(())
}

/// Why the store refused the value of one entry, in the words of the rule.
fn refusal(error: RemoteCredentialError) -> KeyringImportError {
    match error {
        RemoteCredentialError::InvalidValue | RemoteCredentialError::NotHeaderSafe => {
            KeyringImportError::RefusedValue
        }
        other => KeyringImportError::Store(other),
    }
}
