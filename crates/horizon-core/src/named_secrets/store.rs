//! The named secrets of this computer: the names that the settings hold, and the
//! values that its credential store holds under `named/<NAME>`.
//!
//! The settings hold the names and nothing else, so no value is ever written into a
//! settings document. A value lives in the credential store, sealed by the store of
//! the operating system, and it leaves the store in exactly two ways: as the
//! environment of an agent panel that this process starts, and as the body of a
//! secret that the link to a Horizon server carries. Neither path writes it to a
//! file, a log line or an error text.
//!
//! The store is behind the sealed [`RemoteCredentialStore`] trait, so this module
//! reads and writes values the same way through the OS store and through the
//! in-memory store of a test. Every method here runs on a worker thread: a locked
//! keychain or a D-Bus round trip never holds the UI thread.

use std::{
    collections::BTreeSet,
    fmt,
    sync::{Mutex, PoisonError, RwLock},
};

use horizon_browser::remote::CredentialReference;
use zeroize::Zeroizing;

use super::{
    MAX_NAMED_SECRETS, NamedSecretError, NamedSecretState, SecretName, check_value, environment, map_store_error,
};
use crate::remote_browser_credential::{CredentialLocator, RemoteCredentialError, RemoteCredentialStore};

/// The prefix of the id of a named secret, as the server holds it.
pub const NAMED_PREFIX: &str = "named/";

/// The origin that the OS credential store keeps a named secret under. It is not
/// an endpoint of any provider, so a named secret and a provider credential never
/// read each other's item.
pub const NAMED_SECRET_ORIGIN: &str = "horizon:named";

/// Where one named secret lives in the OS credential store: the reference
/// `named/<NAME>`, under an origin of Horizon's own.
#[must_use]
pub fn locator(name: &SecretName) -> CredentialLocator {
    let reference = format!("{NAMED_PREFIX}{name}");
    CredentialLocator {
        origin: NAMED_SECRET_ORIGIN.to_owned(),
        reference: CredentialReference::from(reference.as_str()),
        slot: Some(reference),
    }
}

/// One named secret whose value this computer stored or removed, for a link to a
/// server that holds the same values. It carries the name and never the value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedSecretChange {
    pub name: SecretName,
    /// Whether the value was stored (`true`) or removed.
    pub stored: bool,
}

/// The names and the values of the named secrets of this computer.
pub struct NamedSecretVault {
    /// The names that the settings list, and the names that the link learned from
    /// the server. Read by the UI for its list and by the link for its ids.
    names: RwLock<BTreeSet<SecretName>>,
    /// The OS credential store, or nothing when this computer has none.
    store: Option<Mutex<Box<dyn RemoteCredentialStore + Send>>>,
}

impl fmt::Debug for NamedSecretVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NamedSecretVault")
            .field("names", &self.names())
            .field("store", &self.store.is_some())
            .finish()
    }
}

impl NamedSecretVault {
    /// The values of `store`. A computer without a credential store keeps its
    /// names and holds no value, so a panel of that computer gets no named secret.
    #[must_use]
    pub fn new(store: Option<Box<dyn RemoteCredentialStore + Send>>) -> Self {
        Self {
            names: RwLock::new(BTreeSet::new()),
            store: store.map(Mutex::new),
        }
    }

    /// Whether this computer can keep a value at all.
    #[must_use]
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Replaces the list of names. A name that the rule refuses is dropped with
    /// one line of the log, because the settings of this computer are the person's
    /// own text and one bad name must not stop the others.
    pub fn set_names(&self, names: impl IntoIterator<Item = String>) {
        let mut set = BTreeSet::new();
        for name in names {
            match SecretName::parse(&name) {
                Ok(name) if set.contains(&name) => {}
                Ok(name) if set.len() < MAX_NAMED_SECRETS => {
                    set.insert(name);
                }
                Ok(name) => {
                    tracing::warn!(name = %name, "the settings list more named secrets than this Horizon keeps");
                }
                Err(error) => {
                    tracing::warn!(
                        name = %name,
                        code = error.code(),
                        reason = %error,
                        "a named secret of the settings is refused"
                    );
                }
            }
        }
        *self.names.write().unwrap_or_else(PoisonError::into_inner) = set;
    }

    /// The names of the named secrets of this computer, in name order.
    #[must_use]
    pub fn names(&self) -> Vec<SecretName> {
        self.names
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Takes `name` into the list, as a value of the server does, without writing
    /// the settings: the caller that showed the name to the person saves them.
    pub fn learn(&self, name: &SecretName) {
        let mut names = self.names.write().unwrap_or_else(PoisonError::into_inner);
        if names.len() < MAX_NAMED_SECRETS {
            names.insert(name.clone());
        }
    }

    /// Forgets `name` from the list, as a removal of the server does.
    pub fn forget(&self, name: &SecretName) {
        self.names.write().unwrap_or_else(PoisonError::into_inner).remove(name);
    }

    /// The value of `name`, or `None` when this computer holds none.
    ///
    /// # Errors
    /// A locked or unavailable store, or a store that failed.
    pub fn value(&self, name: &SecretName) -> Result<Option<Zeroizing<Vec<u8>>>, NamedSecretError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        match crate::secret::read_secret_value(store.as_ref(), &locator(name)) {
            Ok(value) => Ok(Some(value)),
            Err(RemoteCredentialError::Missing) => Ok(None),
            Err(error) => Err(map_store_error(name, &error)),
        }
    }

    /// Whether this computer holds a value for `name`.
    ///
    /// # Errors
    /// A locked or unavailable store, or a store that failed.
    pub fn contains(&self, name: &SecretName) -> Result<bool, NamedSecretError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(false);
        };
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        store
            .contains(&locator(name))
            .map_err(|error| map_store_error(name, &error))
    }

    /// Stores `value` under `name`, replacing a value that is there, and takes the
    /// name into the list.
    ///
    /// # Errors
    /// A value that the rule refuses, a locked or unavailable store, or a store
    /// that failed.
    pub fn put(&self, name: &SecretName, value: &[u8]) -> Result<NamedSecretChange, NamedSecretError> {
        check_value(name, value)?;
        let Some(store) = self.store.as_ref() else {
            return Err(NamedSecretError::StoreUnavailable);
        };
        {
            let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
            store
                .put(&locator(name), value)
                .map_err(|error| map_store_error(name, &error))?;
        }
        self.learn(name);
        environment::set(name.clone(), text_of(value));
        Ok(NamedSecretChange {
            name: name.clone(),
            stored: true,
        })
    }

    /// Forgets the value and the name of `name`. A name without a value is already
    /// forgotten.
    ///
    /// # Errors
    /// A locked or unavailable store, or a store that failed.
    pub fn remove(&self, name: &SecretName) -> Result<NamedSecretChange, NamedSecretError> {
        if let Some(store) = self.store.as_ref() {
            let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
            store
                .delete(&locator(name))
                .map_err(|error| map_store_error(name, &error))?;
        }
        self.forget(name);
        environment::remove(name);
        Ok(NamedSecretChange {
            name: name.clone(),
            stored: false,
        })
    }

    /// Reads every value of this computer again and publishes the ones that the
    /// next agent panel inherits. It runs on a worker thread, and it answers the
    /// names that this computer holds a value for.
    ///
    /// # Errors
    /// A locked or unavailable store, or a store that failed. The published values
    /// of the last successful read stay.
    pub fn refresh(&self) -> Result<Vec<SecretName>, NamedSecretError> {
        let mut values = std::collections::BTreeMap::new();
        for name in self.names() {
            match self.value(&name) {
                Ok(Some(value)) => {
                    if let Ok(text) = String::from_utf8(value.to_vec()) {
                        values.insert(name, Zeroizing::new(text));
                    }
                }
                Ok(None) => {}
                Err(error @ (NamedSecretError::Locked | NamedSecretError::StoreUnavailable)) => {
                    environment::publish(values);
                    return Err(error);
                }
                Err(error) => {
                    tracing::warn!(
                        name = %name,
                        code = error.code(),
                        reason = %error,
                        "a named secret of this computer was not read"
                    );
                }
            }
        }
        let mut present = Vec::with_capacity(values.len());
        present.extend(values.keys().cloned());
        environment::publish(values);
        Ok(present)
    }

    /// The state of one name for the settings window: whether this computer holds
    /// its value now.
    ///
    /// # Errors
    /// A locked or unavailable store, or a store that failed.
    pub fn state(&self, name: &SecretName) -> Result<NamedSecretState, NamedSecretError> {
        if !self.has_store() {
            return Ok(NamedSecretState::StoreUnavailable);
        }
        Ok(if self.contains(name)? {
            NamedSecretState::Present
        } else {
            NamedSecretState::NoValue
        })
    }

    /// The ids of the named secrets that this computer holds a value for, as the
    /// server holds them.
    #[must_use]
    pub fn local_ids(&self) -> Vec<String> {
        self.names()
            .into_iter()
            .filter(|name| matches!(self.contains(name), Ok(true)))
            .map(|name| format!("{NAMED_PREFIX}{name}"))
            .collect()
    }
}

/// One value as the text of an environment variable. The value is checked as text
/// before it is stored, so this conversion cannot lose a byte.
fn text_of(value: &[u8]) -> Zeroizing<String> {
    Zeroizing::new(String::from_utf8_lossy(value).into_owned())
}
