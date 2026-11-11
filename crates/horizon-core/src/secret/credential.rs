//! One credential value of a remote browser provider, in the OS credential store
//! of this computer.
//!
//! A value lives under the reference that the provider names, and this computer
//! decides where that reference points: the origin of the provider endpoint and
//! the slot of its binding. Two computers that hold the same provider
//! configuration therefore read the same value, and a value that arrives from the
//! server is written where the code of this computer already reads it.
//!
//! This module is the only place outside the workbench that reads or writes an OS
//! credential value, and it does it through the sealed store trait: no method here
//! returns a value except [`read_secret_value`], whose answer is zeroized, and no
//! text of a value reaches a log line, a refusal or a file.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Mutex, PoisonError},
};

use horizon_browser::remote::{CredentialStoreKind, RemoteBrowserConfig};
use zeroize::Zeroizing;

use super::SecretError;
use crate::remote_browser_credential::{
    CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink,
};

/// The credential references of this computer that only the OS store answers, in
/// reference order, each with every address that its providers use.
///
/// A reference is one name inside a provider: two providers may name the same
/// reference with different endpoints, so one reference can mean two items of the
/// OS store. Both are written with the one value, because the value is bound to
/// the reference and not to one provider.
#[must_use]
pub fn os_locators(remote: &RemoteBrowserConfig) -> BTreeMap<String, Vec<CredentialLocator>> {
    let mut locators: BTreeMap<String, Vec<CredentialLocator>> = BTreeMap::new();
    for profile in remote.providers.values() {
        for reference in profile.authentication.references() {
            let Some(binding) = profile.credential_bindings.get(reference) else {
                continue;
            };
            if binding.store != CredentialStoreKind::OsKeychain {
                continue;
            }
            let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
            let entry = locators.entry(reference.as_str().to_owned()).or_default();
            if !entry.contains(&locator) {
                entry.push(locator);
            }
        }
    }
    locators
}

/// The value of one OS-store item, in a zeroizing buffer.
///
/// # Errors
/// Returns a locked or unavailable store, a missing item, or a value that the
/// sink refuses.
pub fn read_secret_value(
    store: &dyn RemoteCredentialStore,
    locator: &CredentialLocator,
) -> Result<Zeroizing<Vec<u8>>, RemoteCredentialError> {
    let mut sink = ValueSink::default();
    store.with_secret(locator, &mut sink)?;
    sink.value.take().ok_or(RemoteCredentialError::Missing)
}

/// Accepts one value into a zeroizing buffer. The store trait is sealed, so this
/// is the only sink of the sync path.
#[derive(Default)]
struct ValueSink {
    value: Option<Zeroizing<Vec<u8>>>,
}

impl Sealed for ValueSink {}

impl SecretSink for ValueSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.value = Some(Zeroizing::new(bytes.to_vec()));
        Ok(())
    }
}

/// The credential values of this computer: the addresses that its configuration
/// gives to each reference, and the OS credential store that holds them.
pub struct CredentialVault {
    /// The addresses of the references, taken from the remote browser
    /// configuration of this computer. A save of the settings replaces them, and
    /// the link reads them on its own thread.
    locators: Mutex<BTreeMap<String, Vec<CredentialLocator>>>,
    store: Mutex<Box<dyn RemoteCredentialStore + Send>>,
}

impl fmt::Debug for CredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialVault")
            .field("references", &self.references())
            .finish_non_exhaustive()
    }
}

impl CredentialVault {
    /// The values of `store`, with no reference registered yet.
    #[must_use]
    pub fn new(store: Box<dyn RemoteCredentialStore + Send>) -> Self {
        Self {
            locators: Mutex::new(BTreeMap::new()),
            store: Mutex::new(store),
        }
    }

    /// Takes the addresses of the references that `remote` binds to the OS store.
    /// A provider that the person adds or removes applies at the next call.
    pub fn register(&self, remote: &RemoteBrowserConfig) {
        let locators = os_locators(remote);
        *self.locators.lock().unwrap_or_else(PoisonError::into_inner) = locators;
    }

    /// The references that this computer has an address for.
    #[must_use]
    pub fn references(&self) -> Vec<String> {
        self.locators
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// The value of `reference`, or `None` when no address of it holds one.
    ///
    /// # Errors
    /// Returns why the value cannot be read, and refuses a reference that no
    /// provider of this computer binds to the OS store.
    pub fn value(&self, reference: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        let locators = self.locators(reference)?;
        let store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        for locator in &locators {
            match read_secret_value(store.as_ref(), locator) {
                Ok(value) => return Ok(Some(value)),
                Err(RemoteCredentialError::Missing) => {}
                Err(error) => return Err(unavailable(reference, &error)),
            }
        }
        Ok(None)
    }

    /// Writes `value` at every address of `reference`.
    ///
    /// # Errors
    /// Refuses a reference that no provider of this computer binds to the OS
    /// store, and returns why the store refused the value.
    pub fn write(&self, reference: &str, value: &[u8]) -> Result<(), SecretError> {
        let locators = self.locators(reference)?;
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        for locator in &locators {
            store
                .put(locator, value)
                .map_err(|error| unavailable(reference, &error))?;
        }
        Ok(())
    }

    /// Forgets the value at every address of `reference`. An address without a
    /// value is already forgotten.
    ///
    /// # Errors
    /// Refuses a reference that no provider of this computer binds to the OS
    /// store, and returns why the store refused the removal.
    pub fn remove(&self, reference: &str) -> Result<(), SecretError> {
        let locators = self.locators(reference)?;
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        for locator in &locators {
            store.delete(locator).map_err(|error| unavailable(reference, &error))?;
        }
        Ok(())
    }

    /// The references of this computer that hold a value now.
    #[must_use]
    pub fn present(&self) -> Vec<String> {
        let store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        self.locators
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, locators)| {
                locators
                    .iter()
                    .any(|locator| matches!(store.contains(locator), Ok(true)))
            })
            .map(|(reference, _)| reference.clone())
            .collect()
    }

    /// The addresses of `reference`, or the refusal of a reference that no
    /// provider of this computer binds to the OS store.
    fn locators(&self, reference: &str) -> Result<Vec<CredentialLocator>, SecretError> {
        let locators = self.locators.lock().unwrap_or_else(PoisonError::into_inner);
        locators.get(reference).cloned().ok_or_else(|| SecretError::Unsupported {
            reason: format!(
                "No provider of this computer binds the credential reference {reference:?} to the OS credential store"
            ),
        })
    }
}

fn unavailable(reference: &str, error: &RemoteCredentialError) -> SecretError {
    SecretError::Unavailable {
        reason: format!("The credential value of {reference:?}: {error}"),
    }
}
