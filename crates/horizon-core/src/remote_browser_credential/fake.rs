use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use horizon_browser::remote::CredentialStoreKind;
use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret};

/// In-memory stand-in for the OS credential store, for tests and UI previews.
/// It can be locked to exercise the locked-store paths without a platform, and
/// its values are zeroized like the session store's because a preview can
/// receive real entered values. Items are addressed exactly like the real
/// adapter's, by endpoint origin and slot, so two references bound to one
/// slot alias here as they would in the OS store and a binding without a
/// slot is missing.
#[derive(Default)]
pub struct FakeCredentialStore {
    entries: BTreeMap<(String, String), Zeroizing<Vec<u8>>>,
    locked: bool,
    /// A lock that the test flips while the store is already in the worker, as a
    /// Secret Service restart does to the daemon behind a running Horizon.
    shared_lock: Option<Arc<AtomicBool>>,
}

impl FakeCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose lock this handle can flip while another thread holds it.
    #[must_use]
    pub fn with_shared_lock(shared_lock: Arc<AtomicBool>) -> Self {
        Self {
            shared_lock: Some(shared_lock),
            ..Self::default()
        }
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }

    fn guard(&self) -> Result<(), RemoteCredentialError> {
        if self.locked
            || self
                .shared_lock
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            Err(RemoteCredentialError::Locked)
        } else {
            Ok(())
        }
    }

    fn key(locator: &CredentialLocator) -> Result<(String, String), RemoteCredentialError> {
        let slot = locator.slot.as_deref().ok_or(RemoteCredentialError::Missing)?;
        Ok((locator.origin.clone(), slot.to_string()))
    }
}

impl Sealed for FakeCredentialStore {}

impl RemoteCredentialStore for FakeCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::OsKeychain
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        self.guard()?;
        validate_secret(secret)?;
        self.entries
            .insert(Self::key(locator)?, Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        self.guard()?;
        self.entries.remove(&Self::key(locator)?);
        Ok(())
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        self.guard()?;
        Ok(self.entries.contains_key(&Self::key(locator)?))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        self.guard()?;
        let value = self
            .entries
            .get(&Self::key(locator)?)
            .ok_or(RemoteCredentialError::Missing)?;
        sink.accept(value)
    }
}

impl fmt::Debug for FakeCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakeCredentialStore")
            .field("entries", &self.entries.len())
            .field("locked", &self.locked)
            .field("shared_lock", &self.shared_lock.is_some())
            .finish()
    }
}
