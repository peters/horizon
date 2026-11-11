//! A credential store of one private file, for the isolated smoke fixture only.
//!
//! An isolated desktop has no OS credential store of its own: its Secret Service
//! is a bus daemon with no keyring collection behind it, so a procedure that must
//! show a named secret reaching an agent panel has nowhere to put the value. This
//! store keeps the values of the named secrets in one JSON file that the fixture
//! owns, in the private state directory of the fixture.
//!
//! It is used only when `HORIZON_TEST_NAMED_SECRETS_FILE` names that file, which the
//! smoke fixture sets and nothing else does. A Horizon that a person starts has no
//! such variable, so it reads the OS credential store and no file. The values in
//! this file are in clear text, which is why the fixture owns it, why the file is
//! 0600 in a 0700 directory, and why it is deleted with the fixture state.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use horizon_browser::remote::CredentialStoreKind;
use zeroize::Zeroizing;

use super::super::remote_browser_credential::{
    CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret,
};
use crate::AtomicFile;

/// The variable that names the file of the store. Only the smoke fixture sets it.
pub const TEST_FILE_ENV: &str = "HORIZON_TEST_NAMED_SECRETS_FILE";

/// The values of the named secrets, in one private file of a fixture.
pub struct TestFileNamedSecretStore {
    file: AtomicFile,
    values: BTreeMap<String, Zeroizing<String>>,
}

impl TestFileNamedSecretStore {
    /// The store of the file that [`TEST_FILE_ENV`] names, or `None` when this
    /// process names no file.
    ///
    /// # Errors
    /// The file holds something that is not the values of a named secret.
    pub fn from_environment() -> Result<Option<Self>, RemoteCredentialError> {
        match std::env::var_os(TEST_FILE_ENV).filter(|value| !value.is_empty()) {
            Some(path) => Self::at(PathBuf::from(path)).map(Some),
            None => Ok(None),
        }
    }

    /// The store of the file at `path`, which the smoke fixture names.
    ///
    /// # Errors
    /// The file holds something that is not the values of a named secret.
    pub fn at(path: PathBuf) -> Result<Self, RemoteCredentialError> {
        let file = AtomicFile::new(path);
        let text = file
            .read()
            .map_err(|_| RemoteCredentialError::Platform {
                kind: "test_store_read",
            })?
            .unwrap_or_else(|| "{}".to_owned());
        let parsed: BTreeMap<String, String> =
            serde_json::from_str(&text).map_err(|_| RemoteCredentialError::Platform {
                kind: "test_store_format",
            })?;
        Ok(Self {
            file,
            values: parsed
                .into_iter()
                .map(|(slot, value)| (slot, Zeroizing::new(value)))
                .collect(),
        })
    }

    /// The file that holds the values.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }

    fn slot(locator: &CredentialLocator) -> Result<String, RemoteCredentialError> {
        locator.slot.clone().ok_or(RemoteCredentialError::Missing)
    }

    fn save(&self) -> Result<(), RemoteCredentialError> {
        let plain: BTreeMap<&str, &str> = self
            .values
            .iter()
            .map(|(slot, value)| (slot.as_str(), value.as_str()))
            .collect();
        let document = serde_json::to_vec(&plain).map_err(|_| RemoteCredentialError::Platform {
            kind: "test_store_format",
        })?;
        self.file.write(&document).map_err(|_| RemoteCredentialError::Platform {
            kind: "test_store_write",
        })
    }
}

impl Sealed for TestFileNamedSecretStore {}

impl RemoteCredentialStore for TestFileNamedSecretStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::OsKeychain
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        validate_secret(secret)?;
        let slot = Self::slot(locator)?;
        let text = String::from_utf8(secret.to_vec()).map_err(|_| RemoteCredentialError::InvalidValue)?;
        self.values.insert(slot, Zeroizing::new(text));
        self.save()
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        let slot = Self::slot(locator)?;
        if self.values.remove(&slot).is_none() {
            return Ok(());
        }
        self.save()
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        Ok(self.values.contains_key(&Self::slot(locator)?))
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let value = self
            .values
            .get(&Self::slot(locator)?)
            .ok_or(RemoteCredentialError::Missing)?;
        sink.accept(value.as_bytes())
    }
}

impl fmt::Debug for TestFileNamedSecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestFileNamedSecretStore")
            .field("path", &self.file.path())
            .field("values", &self.values.len())
            .finish()
    }
}
