use std::fmt;
use std::sync::Arc;

use horizon_browser::remote::CredentialStoreKind;
use keyring_core::{CredentialStore, Entry, Error as KeyringError};

use zeroize::Zeroizing;

use super::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, validate_secret};

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

/// Decides whether an item exists without reading its value or prompting.
///
/// `keyring-core`'s generic `get_credential` is not value-free on every
/// backend (the macOS store loads the password to answer it, and the Secret
/// Service store unlocks locked items first), so each platform supplies the
/// metadata-only probe its store supports.
type PresenceProbe = fn(&CredentialStore, &str) -> Result<bool, RemoteCredentialError>;

/// OS credential store adapter: macOS Keychain, Windows Credential Manager,
/// Linux Secret Service over D-Bus. Items are addressed by endpoint origin
/// and slot, so a value bound to one endpoint is invisible to another.
pub struct KeyringCredentialStore {
    store: Arc<CredentialStore>,
    probe: PresenceProbe,
}

impl KeyringCredentialStore {
    /// Connect to the platform store. Never prompts and never creates items.
    ///
    /// # Errors
    /// [`RemoteCredentialError::StoreUnavailable`] when the platform has no
    /// store (or none is reachable), otherwise the mapped platform failure.
    pub fn open() -> Result<Self, RemoteCredentialError> {
        platform::open().map(|store| Self {
            store,
            probe: platform::probe,
        })
    }

    /// Wrap an in-memory keyring-core store for tests. Presence goes through
    /// the store's attribute search, like the macOS keychain.
    #[cfg(test)]
    pub(crate) fn with_store(store: Arc<CredentialStore>) -> Self {
        Self {
            store,
            probe: search_probe,
        }
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

    fn user(locator: &CredentialLocator) -> Result<String, RemoteCredentialError> {
        let slot = locator.slot.as_deref().ok_or(RemoteCredentialError::Missing)?;
        Ok(format!("{}|{slot}", locator.origin))
    }

    fn entry(&self, locator: &CredentialLocator) -> Result<Entry, RemoteCredentialError> {
        self.store
            .build(KEYRING_SERVICE, &Self::user(locator)?, None)
            .map_err(|error| map_error(&error))
    }
}

impl Sealed for KeyringCredentialStore {}

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
        (self.probe)(self.store.as_ref(), &Self::user(locator)?)
    }

    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError> {
        let value = Zeroizing::new(self.entry(locator)?.get_secret().map_err(|error| map_error(&error))?);
        sink.accept(&value)
    }
}

impl fmt::Debug for KeyringCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyringCredentialStore")
            .field("vendor", &self.store.vendor())
            .finish_non_exhaustive()
    }
}

/// Presence through the store's attribute search, which returns specifiers
/// only: on macOS the search loads item attributes and never the password.
/// Stores may match loosely, so only an exact specifier counts.
#[cfg(any(test, target_os = "macos"))]
fn search_probe(store: &CredentialStore, user: &str) -> Result<bool, RemoteCredentialError> {
    let spec = std::collections::HashMap::from([("service", KEYRING_SERVICE), ("user", user)]);
    match store.search(&spec) {
        Ok(entries) => Ok(entries.iter().any(|entry| {
            entry
                .get_specifiers()
                .is_some_and(|(service, account)| service == KEYRING_SERVICE && account == user)
        })),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(error) => Err(map_error(&error)),
    }
}

/// Map platform errors to the typed, value-free error. The platform message
/// stays in a local trace so diagnostics exist without reaching callers.
pub(super) fn map_error(error: &KeyringError) -> RemoteCredentialError {
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
    use std::collections::HashMap;

    use secret_service::{EncryptionType, blocking::SecretService};

    use super::{Arc, CredentialStore, KEYRING_SERVICE, RemoteCredentialError, map_error};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        zbus_secret_service_keyring_store::Store::new()
            .map(|store| store as Arc<CredentialStore>)
            .map_err(|error| map_error(&error))
    }

    /// Ask the Secret Service which items match, without unlocking any: a
    /// match that exists only in a locked collection reports `Locked`
    /// instead of prompting the user, which the keyring store's own search
    /// would do.
    pub(super) fn probe(_store: &CredentialStore, user: &str) -> Result<bool, RemoteCredentialError> {
        let service = SecretService::connect(EncryptionType::Dh).map_err(|error| map_service_error(&error))?;
        let found = service
            .search_items(HashMap::from([("service", KEYRING_SERVICE), ("username", user)]))
            .map_err(|error| map_service_error(&error))?;
        if !found.unlocked.is_empty() {
            Ok(true)
        } else if found.locked.is_empty() {
            Ok(false)
        } else {
            Err(RemoteCredentialError::Locked)
        }
    }

    fn map_service_error(error: &secret_service::Error) -> RemoteCredentialError {
        match error {
            secret_service::Error::Locked => RemoteCredentialError::Locked,
            secret_service::Error::Unavailable => RemoteCredentialError::StoreUnavailable,
            _ => {
                tracing::warn!("Secret Service presence probe failed");
                RemoteCredentialError::Platform { kind: "secret_service" }
            }
        }
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

    /// The keychain search loads attributes only, never item data.
    pub(super) use super::search_probe as probe;
}

#[cfg(windows)]
mod platform {
    use keyring_core::Error as KeyringError;

    use super::{Arc, CredentialStore, KEYRING_SERVICE, RemoteCredentialError, map_error};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        windows_native_keyring_store::Store::new()
            .map(|store| store as Arc<CredentialStore>)
            .map_err(|error| map_error(&error))
    }

    /// Credential Manager has no metadata-only read: `CredReadW` returns the
    /// whole record in API-owned memory that the store frees at once. Reading
    /// the attributes of the one target keeps that to this item instead of
    /// enumerating every credential the user owns.
    pub(super) fn probe(store: &CredentialStore, user: &str) -> Result<bool, RemoteCredentialError> {
        let entry = store
            .build(KEYRING_SERVICE, user, None)
            .map_err(|error| map_error(&error))?;
        match entry.get_attributes() {
            Ok(_) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(map_error(&error)),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::{Arc, CredentialStore, RemoteCredentialError};

    pub(super) fn open() -> Result<Arc<CredentialStore>, RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }

    pub(super) fn probe(_store: &CredentialStore, _user: &str) -> Result<bool, RemoteCredentialError> {
        Err(RemoteCredentialError::StoreUnavailable)
    }
}
