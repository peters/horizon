//! Provider credentials for remote browser sessions: where bound values live
//! and how they become one authorization header, without ever returning the
//! secret to callers.
//!
//! Values reach the transport through [`ProviderAuthorization`], whose only
//! exit is the header value. Stores expose presence, locked state and a
//! sink-based copy; both the store and sink traits are sealed, so no code
//! outside this crate can implement a sink that captures bytes. Every
//! intermediate buffer is a `zeroize` type that is wiped when dropped, on
//! success and on every error path alike. Nothing here serializes, logs or
//! places a value in the process environment, so agent panels cannot inherit
//! it.

mod fake;
mod keyring_store;
mod session;
mod workbench;

use std::fmt;

use base64::Engine as _;
use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAuthentication,
    RemoteProviderProfile,
};
use zeroize::Zeroizing;

pub use fake::FakeCredentialStore;
pub use keyring_store::{KEYRING_SERVICE, KeyringCredentialStore, KeyringStoreAvailability};
pub use session::SessionCredentialStore;
pub use workbench::{CredentialWorkbench, KeychainState, NoticeKind, SharedStore, StoreOpener, WorkbenchNotice};

/// Largest accepted secret. Provider keys are far smaller; the bound stops a
/// pasted file from becoming a header.
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

mod private {
    /// Sealing supertrait: only this crate's stores and sinks exist.
    pub trait Sealed {}
}

pub(crate) use private::Sealed;

/// Where one bound value lives: the endpoint origin it may be sent to, the
/// reference the configuration uses, and the OS-store slot when persisted.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CredentialLocator {
    pub origin: String,
    pub reference: CredentialReference,
    pub slot: Option<String>,
}

impl CredentialLocator {
    #[must_use]
    pub fn new(endpoint: &ControlEndpoint, reference: &CredentialReference, binding: &CredentialBinding) -> Self {
        Self {
            origin: endpoint.origin(),
            reference: reference.clone(),
            slot: binding.slot.clone(),
        }
    }
}

/// Destination for one copy of a secret. Sealed: implementations live in this
/// crate and must not retain the bytes after [`SecretSink::accept`] returns.
pub trait SecretSink: Sealed {
    /// # Errors
    /// Returns a typed failure when the bytes are unusable for the sink.
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError>;
}

/// Storage for bound values. Sealed; no method returns secret bytes.
pub trait RemoteCredentialStore: Sealed {
    fn kind(&self) -> CredentialStoreKind;

    /// # Errors
    /// Locked store, platform failure, or an oversized or empty value.
    fn put(&mut self, locator: &CredentialLocator, secret: &[u8]) -> Result<(), RemoteCredentialError>;

    /// Missing items succeed as already absent.
    /// # Errors
    /// Locked store or platform failure.
    fn delete(&mut self, locator: &CredentialLocator) -> Result<(), RemoteCredentialError>;

    /// # Errors
    /// Locked store or platform failure.
    fn contains(&self, locator: &CredentialLocator) -> Result<bool, RemoteCredentialError>;

    /// Copy the value into `sink` only.
    /// # Errors
    /// Locked store, missing item, platform failure, or sink failure.
    fn with_secret(&self, locator: &CredentialLocator, sink: &mut dyn SecretSink) -> Result<(), RemoteCredentialError>;
}

/// Typed, value-free failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteCredentialError {
    #[error("credential store is locked")]
    Locked,
    #[error("credential is missing")]
    Missing,
    #[error("no OS credential store is available on this computer")]
    StoreUnavailable,
    #[error("credential value is empty or larger than {MAX_SECRET_BYTES} bytes")]
    InvalidValue,
    #[error("credential value is not printable ASCII without control characters")]
    NotHeaderSafe,
    #[error("a Basic authentication username must not contain a colon")]
    InvalidUsername,
    #[error("a Bearer token may only contain letters, digits, - . _ ~ + / and trailing =")]
    InvalidBearerToken,
    #[error("credential store failed: {kind}")]
    Platform { kind: &'static str },
    /// The OS store has not answered yet; try again after the next poll.
    #[error("credential store is still being checked")]
    Checking,
}

/// Readiness of one reference for display. Never carries a value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialState {
    Present,
    Missing,
    Locked,
    StoreUnavailable,
    /// An OS-store probe is in flight.
    Checking,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialReadiness {
    pub reference: CredentialReference,
    pub store: CredentialStoreKind,
    pub state: CredentialState,
}

/// The stores a resolver may consult. The OS store is optional because a
/// computer without one must still run session-only credentials.
pub struct CredentialStores<'a> {
    pub session: &'a dyn RemoteCredentialStore,
    pub os_keychain: Option<&'a dyn RemoteCredentialStore>,
}

impl CredentialStores<'_> {
    fn store_for(&self, kind: CredentialStoreKind) -> Result<&dyn RemoteCredentialStore, RemoteCredentialError> {
        match kind {
            CredentialStoreKind::Session => Ok(self.session),
            CredentialStoreKind::OsKeychain => self.os_keychain.ok_or(RemoteCredentialError::StoreUnavailable),
        }
    }
}

/// Presence of every reference a provider needs, in authentication order.
/// Consulting stores never returns a value and never contacts the provider.
/// A store that fails is reported as unavailable, not as a missing value.
#[must_use]
pub fn readiness(profile: &RemoteProviderProfile, stores: &CredentialStores<'_>) -> Vec<CredentialReadiness> {
    profile
        .authentication
        .references()
        .into_iter()
        .map(|reference| {
            let Some(binding) = profile.credential_bindings.get(reference) else {
                return CredentialReadiness {
                    reference: reference.clone(),
                    store: CredentialStoreKind::Session,
                    state: CredentialState::Missing,
                };
            };
            let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
            let state = match stores
                .store_for(binding.store)
                .and_then(|store| store.contains(&locator))
            {
                Ok(true) => CredentialState::Present,
                Ok(false) | Err(RemoteCredentialError::Missing) => CredentialState::Missing,
                Err(RemoteCredentialError::Locked) => CredentialState::Locked,
                Err(RemoteCredentialError::Checking) => CredentialState::Checking,
                Err(_) => CredentialState::StoreUnavailable,
            };
            CredentialReadiness {
                reference: reference.clone(),
                store: binding.store,
                state,
            }
        })
        .collect()
}

/// Authorization header for one provider, bound to its endpoint origin.
///
/// The header value is the only exit. Debug output is redacted and the buffer
/// is zeroized on drop.
pub struct ProviderAuthorization {
    origin: String,
    header: Zeroizing<String>,
}

impl ProviderAuthorization {
    /// The origin the header may be sent to; the transport must refuse others.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    #[must_use]
    pub fn header_value(&self) -> &str {
        &self.header
    }
}

impl fmt::Debug for ProviderAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderAuthorization")
            .field("origin", &self.origin)
            .field("header", &"<redacted>")
            .finish()
    }
}

/// Which reference failed, without its value.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("credential `{}`: {error}", reference.as_str())]
pub struct ResolveError {
    pub reference: CredentialReference,
    pub error: RemoteCredentialError,
}

/// Build the authorization header from bindings, or `None` for `kind: none`.
///
/// Values are copied through a private sink into zeroizing buffers, checked
/// against the grammar of their scheme, and combined in memory; nothing is
/// returned but the finished header, and every intermediate buffer is wiped
/// on the success path and on every error path.
///
/// # Errors
/// The first reference that is unbound, missing, locked, unavailable or
/// malformed for its scheme.
pub fn resolve_authorization(
    profile: &RemoteProviderProfile,
    stores: &CredentialStores<'_>,
) -> Result<Option<ProviderAuthorization>, ResolveError> {
    let origin = profile.endpoint.origin();
    let header = match &profile.authentication {
        RemoteAuthentication::None {} => return Ok(None),
        RemoteAuthentication::Basic {
            username_ref,
            password_ref,
        } => {
            let username = fetch(profile, stores, username_ref)?;
            if username.contains(':') {
                return Err(ResolveError {
                    reference: username_ref.clone(),
                    error: RemoteCredentialError::InvalidUsername,
                });
            }
            let password = fetch(profile, stores, password_ref)?;
            let joined = Zeroizing::new(format!("{}:{}", username.as_str(), password.as_str()));
            let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(joined.as_bytes()));
            Zeroizing::new(format!("Basic {}", encoded.as_str()))
        }
        RemoteAuthentication::Bearer { token_ref } => {
            let token = fetch(profile, stores, token_ref)?;
            if !is_token68(&token) {
                return Err(ResolveError {
                    reference: token_ref.clone(),
                    error: RemoteCredentialError::InvalidBearerToken,
                });
            }
            Zeroizing::new(format!("Bearer {}", token.as_str()))
        }
    };
    Ok(Some(ProviderAuthorization { origin, header }))
}

fn fetch(
    profile: &RemoteProviderProfile,
    stores: &CredentialStores<'_>,
    reference: &CredentialReference,
) -> Result<Zeroizing<String>, ResolveError> {
    let fail = |error| ResolveError {
        reference: reference.clone(),
        error,
    };
    let binding = profile
        .credential_bindings
        .get(reference)
        .ok_or_else(|| fail(RemoteCredentialError::Missing))?;
    let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
    let store = stores.store_for(binding.store).map_err(fail)?;
    let mut sink = HeaderSafeSink::default();
    store.with_secret(&locator, &mut sink).map_err(fail)?;
    sink.value.take().ok_or_else(|| fail(RemoteCredentialError::Missing))
}

/// RFC 7235 `token68`: the only shape a Bearer credential may take.
fn is_token68(token: &str) -> bool {
    let trimmed = token.trim_end_matches('=');
    !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/'))
}

/// Accepts one printable-ASCII value into a zeroizing buffer.
#[derive(Default)]
struct HeaderSafeSink {
    value: Option<Zeroizing<String>>,
}

impl Sealed for HeaderSafeSink {}

impl SecretSink for HeaderSafeSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        validate_secret(bytes)?;
        if !bytes.iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
            return Err(RemoteCredentialError::NotHeaderSafe);
        }
        self.value = Some(Zeroizing::new(String::from_utf8_lossy(bytes).into_owned()));
        Ok(())
    }
}

pub(crate) fn validate_secret(bytes: &[u8]) -> Result<(), RemoteCredentialError> {
    if bytes.is_empty() || bytes.len() > MAX_SECRET_BYTES {
        return Err(RemoteCredentialError::InvalidValue);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
