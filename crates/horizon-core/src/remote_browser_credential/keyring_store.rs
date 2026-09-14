use std::fmt;
use std::sync::Arc;

use horizon_browser::remote::CredentialStoreKind;
use keyring_core::{CredentialStore, Entry, Error as KeyringError};

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, SecretSink, scrub, validate_secret};

/// Service name for every remote-provider item, distinct from any routine
/// login items so the two features can never read each other's entries.
pub const KEYRING_SERVICE: &str = "horizon-remote-browser";

/// Whether this computer has a usable platform store, decided without
/// creating, reading or prompting for any item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyringStoreAvailability {
    Available,
    Unavailable { kind: &'static str },
}

/// OS credential store adapter: macOS Keychain, Windows Credential Manager,
/// Linux Secret Service over D-Bus. Items are addressed by endpoint origin
/// and slot, so a value bound to one endpoint is invisible to another.
pub struct KeyringCredentialStore {
    store: Arc<CredentialStore>,
}

impl KeyringCredentialStore {
    /// Connect to the platform store. Never prompts and never creates items.
    ///
    /// # Errors
    /// [`RemoteCredentialError::StoreUnavailable`] when the platform has no
    /// store (or none is reachable), otherwise the mapped platform failure.
    pub fn open() -> Result<Self, RemoteCredentialError> {
        platform::open().map(|store| Self { store })
    }

    /// Probe availability without touching any item.
    #[must_use]
    pub fn availability() -> KeyringStoreAvailability {
        match platform::open() {
            Ok(_) => KeyringStoreAvailability::Available,
            Err(RemoteCredentialError::Platform { kind }) => KeyringStoreAvailability::Unavailable { kind },
            Err(RemoteCredentialError::Locked) => KeyringStoreAvailability::Unavailable { kind: "locked" },
            Err(_) => KeyringStoreAvailability::Unavailable { kind: "no_store" },
        }
    }

    fn entry(&self, locator: &CredentialLocator) -> Result<Entry, RemoteCredentialError> {
        let slot = locator.slot.as_deref().ok_or(RemoteCredentialError::Missing)?;
        let user = format!("{}|{slot}", locator.origin);
        self.store
            .build(KEYRING_SERVICE, &user, None)
            .map_err(|error| map_error(&error))
    }
}

impl RemoteCredentialStore for KeyringCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::OsKeychain
    }

    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError> {
        validate_secret(secret)?;
        self.entry(locator)?
            .set_secret(secret)
            .map_err(|error| map_error(&error))
    }

    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError> {
        match self.entry(locator)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(map_error(&error)),
        }
    }

    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError> {
        let entry = self.entry(locator)?;
        match entry.get_credential() {
            Ok(_) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(map_error(&error)),
        }
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let mut value = self.entry(locator)?.get_secret().map_err(|error| map_error(&error))?;
        let result = sink.accept(&value);
        scrub(&mut value);
        result
    }
}

impl fmt::Debug for KeyringCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyringCredentialStore")
            .field("vendor", &self.store.vendor())
            .finish()
    }
}

/// Map platform errors to the typed, value-free error. The platform message
/// stays in a local trace so diagnostics exist without reaching callers.
fn map_error(error: &KeyringError) -> RemoteCredentialError {
    let kind = match error {
        KeyringError::NoEntry => return RemoteCredentialError::Missing,
        KeyringError::NoStorageAccess(_) => return RemoteCredentialError::Locked,
        KeyringError::NoDefaultStore | KeyringError::NotSupportedByStore(_) => {
            return RemoteCredentialError::StoreUnavailable;
        }
        KeyringError::TooLong(..) | KeyringError::Invalid(..) => return RemoteCredentialError::InvalidValue,
        KeyringError::PlatformFailure(_) => "platform_failure",
        KeyringError::BadEncoding(_) => "bad_encoding",
        KeyringError::BadDataFormat(..) => "bad_data_format",
        KeyringError::BadStoreFormat(_) => "bad_store_format",
        KeyringError::Ambiguous(_) => "ambiguous",
        _ => "unknown",
    };
    tracing::warn!(kind, "OS credential store operation failed");
    RemoteCredentialError::Platform { kind }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Arc, CredentialStore, RemoteCredentialError, map_error};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        zbus_secret_service_keyring_store::Store::new()
            .map(|store| store as Arc<CredentialStore>)
            .map_err(|error| map_error(&error))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Arc, CredentialStore, RemoteCredentialError, map_error};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        apple_native_keyring_store::keychain::Store::new()
            .map(|store| store as Arc<CredentialStore>)
            .map_err(|error| map_error(&error))
    }
}

#[cfg(windows)]
mod platform {
    use super::{Arc, CredentialStore, RemoteCredentialError, map_error};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        windows_native_keyring_store::Store::new()
            .map(|store| store as Arc<CredentialStore>)
            .map_err(|error| map_error(&error))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::{Arc, CredentialStore, RemoteCredentialError};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }
}
