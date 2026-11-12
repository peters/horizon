//! One import of the keyring: the work that a click starts.
//!
//! The work of a click runs on a worker thread of the Settings tab, so the UI thread
//! never reads a keyring, never starts a program and never waits for one.
//!
//! Two clicks reach this module. The first reads every entry that the keyring of
//! `platform` can name, writes the rows of the remote browser references of these
//! settings, and answers the networks of an older Horizon whose OAuth client the
//! keyring holds, as names only. The second reads the same entries again, writes the
//! named secrets of one network that this computer has no value for, and keeps no
//! value of a network that the person did not ask for.

use std::path::PathBuf;
use std::sync::Arc;

use super::outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport, TailnetReport};
use super::plan::KeyringImportPlan;
use super::readers::{Bound, Platform, TAILNET_API_SERVICE, TAILNET_SERVICE};
use super::run;
use super::tailnet::{self, Offered};
use crate::bounded_program::ProgramEnvironment;
use crate::named_secrets::NamedSecretVault;
use crate::remote_browser_credential::SharedStore;

/// Where an older Horizon kept its saved networks, and the named secrets of this
/// computer that the offer asks about.
pub struct Tailnets {
    old: PathBuf,
    vault: Arc<NamedSecretVault>,
}

impl Tailnets {
    /// The older Horizon's folder of saved networks, under the home of this build,
    /// and the named secrets of this computer.
    #[must_use]
    pub fn new(old: PathBuf, vault: Arc<NamedSecretVault>) -> Self {
        Self { old, vault }
    }
}

impl std::fmt::Debug for Tailnets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Tailnets")
            .field("old", &self.old)
            .finish_non_exhaustive()
    }
}

/// The work of the first click: read every entry that the keyring of `platform` can
/// name, write the rows that these settings name, and answer the networks of the
/// older Horizon whose OAuth client the keyring holds, as names only.
///
/// `tailnets` is `None` in a test of the remote browser rows alone, and in a build
/// without the named secrets of the older Horizon.
///
/// # Errors
/// The typed refusal of the first rule that stops the import: a keyring that this
/// computer cannot read, an entry that does not parse, a reference that breaks the
/// reference rule, a value that the store refuses, or a secrets file that cannot be
/// read or written.
pub fn import_from_keyring(
    platform: Platform,
    plan: &KeyringImportPlan,
    environment: &ProgramEnvironment,
    bound: Bound,
    store: &SharedStore,
    tailnets: Option<&Tailnets>,
) -> Result<KeyringImportOutcome, KeyringImportError> {
    if plan.is_empty() && tailnets.is_none() {
        return Err(KeyringImportError::NothingToDo);
    }
    let entries = platform.entries(plan, environment, bound)?;
    let mut report = KeyringImportReport::default();
    let mut offer = None;
    if let Some(tailnets) = tailnets {
        let read = read_tailnets(platform, environment, bound, tailnets)?;
        report.unused += read.unused;
        offer = Some(tailnet::offer_of(&read.offered));
        // The values of the offered networks are dropped here: the first click keeps
        // the names, and the second click reads the values again.
        drop(read);
    }
    // The read of the keyring waits on a program for up to the bound, so this lock
    // is taken after it and held for the writes alone.
    let mut held = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let written = run::import(entries, plan, held.as_mut())?;
    drop(held);
    report.imported += written.report.imported;
    report.already_here += written.report.already_here;
    report.unused += written.report.unused;
    let changes = written.changes;
    Ok(KeyringImportOutcome {
        report,
        changes,
        offer,
        created: Vec::new(),
        created_report: TailnetReport::default(),
    })
}

/// The work of the second click: the named secrets of the network `only`, which is
/// every offered network when `only` is `None`, written through the named secret path
/// of this computer.
///
/// A name that this computer already has a value for is never written over, so a
/// value that a person typed survives this import.
///
/// # Errors
/// The typed refusal of the first rule that stops the import, and the failure of the
/// named secrets of this computer.
pub fn create_tailnets(
    platform: Platform,
    environment: &ProgramEnvironment,
    bound: Bound,
    tailnets: &Tailnets,
    only: Option<&str>,
) -> Result<KeyringImportOutcome, KeyringImportError> {
    let read = read_tailnets(platform, environment, bound, tailnets)?;
    let mut report = TailnetReport {
        unused: read.unused,
        ..TailnetReport::default()
    };
    let mut created = Vec::new();
    for offered in &read.offered {
        let line = &offered.line;
        if only.is_some_and(|only| only != line.net()) {
            continue;
        }
        report.networks += 1;
        for (index, name) in line.names().iter().enumerate() {
            if line.present()[index] {
                report.already_here += 1;
                continue;
            }
            let Some(value) = offered.value(index) else {
                continue;
            };
            let change = tailnets
                .vault
                .put(name, value)
                .map_err(|_| KeyringImportError::SecretsNotReadable)?;
            report.created += 1;
            created.push(change);
        }
    }
    Ok(KeyringImportOutcome {
        report: KeyringImportReport {
            unused: read.unused,
            ..KeyringImportReport::default()
        },
        changes: Vec::new(),
        offer: Some(tailnet::offer_of(&read.offered)),
        created,
        created_report: report,
    })
}

/// What one reading of the older Horizon's networks answered: the offer, the values
/// behind it, and the count of entries that name no client.
struct Read {
    offered: Vec<Offered>,
    unused: usize,
}

/// Read the two tailnet services of the keyring and the saved networks beside them,
/// and answer the offer of the networks whose OAuth client the keyring holds.
fn read_tailnets(
    platform: Platform,
    environment: &ProgramEnvironment,
    bound: Bound,
    tailnets: &Tailnets,
) -> Result<Read, KeyringImportError> {
    let saved = tailnet::saved(&tailnets.old)?;
    let mut entries = Vec::new();
    for (service, accounts) in [
        (TAILNET_API_SERVICE, saved.api_accounts()),
        (TAILNET_SERVICE, saved.enrollment_accounts()),
    ] {
        entries.extend(platform.service(service, &accounts, environment, bound)?);
    }
    let (clients, unused) = tailnet::clients(saved.networks(), &entries);
    let mut absent = 0_usize;
    let (offered, _refused) = tailnet::offered(&clients, |name| {
        if let Ok(present) = tailnets.vault.contains(name) {
            return present;
        }
        absent += 1;
        true
    });
    if absent > 0 {
        return Err(KeyringImportError::SecretsNotReadable);
    }
    Ok(Read { offered, unused })
}
