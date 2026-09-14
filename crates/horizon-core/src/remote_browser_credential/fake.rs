use std::collections::BTreeMap;
use std::fmt;

use horizon_browser::remote::CredentialStoreKind;
use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret};

/// In-memory stand-in for the OS credential store, for tests and UI previews.
/// It can be locked to exercise the locked-store paths without a platform, and
/// its values are zeroized like the session store's because a preview can
/// receive real entered values.
#[derive(Default)]
pub struct FakeCredentialStore {
    entries: BTreeMap<CredentialLocator, Zeroizing<Vec<u8>>>,
    locked: bool,
}

impl FakeCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }

    fn guard(&self) -> Result<(), RemoteCredentialError> {
        if self.locked {
            Err(RemoteCredentialError::Locked)
        } else {
            Ok(())
        }
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
        self.entries.insert(locator.clone(), Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        self.guard()?;
        self.entries.remove(locator);
        Ok(())
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        self.guard()?;
        Ok(self.entries.contains_key(locator))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        self.guard()?;
        let value = self.entries.get(locator).ok_or(RemoteCredentialError::Missing)?;
        sink.accept(value)
    }
}

impl fmt::Debug for FakeCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakeCredentialStore")
            .field("entries", &self.entries.len())
            .field("locked", &self.locked)
            .finish()
    }
}
