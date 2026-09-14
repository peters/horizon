use std::collections::BTreeMap;
use std::fmt;

use horizon_browser::remote::CredentialStoreKind;

use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret};

/// Session-only values entered in Horizon. Held in memory for this process,
/// keyed by endpoint origin and reference, zeroized on replace, delete, clear and drop.
/// Never serialized, never placed in the environment.
#[derive(Default)]
pub struct SessionCredentialStore {
    values: BTreeMap<(String, String), Zeroizing<Vec<u8>>>,
}

impl SessionCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every value; each buffer is zeroized as it drops.
    pub fn clear(&mut self) {
        self.values.clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn key(locator: &CredentialLocator) -> (String, String) {
        (locator.origin.clone(), locator.reference.as_str().to_string())
    }
}

impl Sealed for SessionCredentialStore {}

impl RemoteCredentialStore for SessionCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::Session
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        validate_secret(secret)?;
        self.values.insert(Self::key(locator), Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        self.values.remove(&Self::key(locator));
        Ok(())
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        Ok(self.values.contains_key(&Self::key(locator)))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let value = self
            .values
            .get(&Self::key(locator))
            .ok_or(RemoteCredentialError::Missing)?;
        sink.accept(value)
    }
}

impl Drop for SessionCredentialStore {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for SessionCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCredentialStore")
            .field("entries", &self.values.len())
            .finish()
    }
}
