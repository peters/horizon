//! One credential store that two owners reach: the vault of this computer on the
//! link thread, and the caller or test that reads the same items.
//!
//! The store trait is sealed, so the only way to give two owners one store is a
//! wrapper of this crate. Every call takes the lock for the length of that call and
//! never returns a value without a sink.

use std::sync::{Arc, Mutex, PoisonError};

use horizon_browser::remote::CredentialStoreKind;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink};

/// One store behind a lock, with a handle that another owner keeps.
pub struct SharedCredentialStore<S> {
    inner: Arc<Mutex<S>>,
}

impl<S> SharedCredentialStore<S> {
    /// The store behind `inner`.
    #[must_use]
    pub fn new(inner: Arc<Mutex<S>>) -> Self {
        Self { inner }
    }

    /// The same store, for another owner.
    #[must_use]
    pub fn handle(&self) -> Arc<Mutex<S>> {
        Arc::clone(&self.inner)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, S> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<S> std::fmt::Debug for SharedCredentialStore<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SharedCredentialStore").finish_non_exhaustive()
    }
}

impl<S: RemoteCredentialStore> Sealed for SharedCredentialStore<S> {}

impl<S: RemoteCredentialStore> RemoteCredentialStore for SharedCredentialStore<S> {
    fn kind(&self) -> CredentialStoreKind {
        self.lock().kind()
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        self.lock().put(locator, secret)
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        self.lock().delete(locator)
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        self.lock().contains(locator)
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        self.lock().with_secret(locator, sink)
    }
}
