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
//! writes a value into the process environment. Environment-backed bindings
//! are read at capture; normal child-process environment inheritance is unchanged.

mod environment;
mod fake;
mod keyring_store;
mod resolved;
mod session;
mod workbench;

use std::fmt;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteBrowserConfig,
    RemoteProviderProfile,
};
use horizon_browser_control::remote_authorization::{CredentialValueError, authorization_header, header_safe_value};
use zeroize::Zeroizing;

pub use environment::EnvironmentCredentialStore;
pub use fake::FakeCredentialStore;
pub use keyring_store::{KEYRING_SERVICE, KeyringCredentialStore, KeyringStoreAvailability};
pub use resolved::ReadCredentialStore;
pub use session::SessionCredentialStore;
pub use workbench::{
    CredentialValueReply, CredentialWorkbench, KeychainState, NoticeKind, SharedStore, StoreOpener, WorkbenchNotice,
    credential_destination,
};

pub use horizon_browser_control::remote_authorization::MAX_SECRET_BYTES;

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

impl From<CredentialValueError> for RemoteCredentialError {
    fn from(error: CredentialValueError) -> Self {
        match error {
            CredentialValueError::InvalidValue => Self::InvalidValue,
            CredentialValueError::NotHeaderSafe => Self::NotHeaderSafe,
            CredentialValueError::InvalidUsername => Self::InvalidUsername,
            CredentialValueError::InvalidBearerToken => Self::InvalidBearerToken,
        }
    }
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
/// computer without one must still run session-only credentials. The
/// environment store is optional so tests can omit it; production always
/// supplies the launch snapshot. There is no implicit fallback between stores.
pub struct CredentialStores<'a> {
    pub session: &'a dyn RemoteCredentialStore,
    pub os_keychain: Option<&'a dyn RemoteCredentialStore>,
    pub environment: Option<&'a dyn RemoteCredentialStore>,
}

impl CredentialStores<'_> {
    fn store_for(&self, kind: CredentialStoreKind) -> Result<&dyn RemoteCredentialStore, RemoteCredentialError> {
        match kind {
            CredentialStoreKind::Session => Ok(self.session),
            CredentialStoreKind::OsKeychain => self.os_keychain.ok_or(RemoteCredentialError::StoreUnavailable),
            CredentialStoreKind::Environment => self.environment.ok_or(RemoteCredentialError::StoreUnavailable),
        }
    }
}

/// The provider profile a configured target authenticates against, or `None`
/// when the target or its provider is not configured. A caller that must read
/// values from the OS store before the request is built resolves them for this
/// profile; [`build_remote_session_request`](crate::browser::build_remote_session_request)
/// refuses an unconfigured target or provider itself, with its own code and text.
#[must_use]
pub fn target_provider_profile<'a>(
    remote: &'a RemoteBrowserConfig,
    target_name: &str,
) -> Option<&'a RemoteProviderProfile> {
    let target = remote.targets.get(target_name)?;
    remote.providers.get(&target.provider)
}

/// The references of `profile` whose value only the OS store can answer, in
/// authentication order. Each must be read on the workbench thread before the
/// request is built, and offered back to it as a [`ReadCredentialStore`].
#[must_use]
pub fn os_store_references(profile: &RemoteProviderProfile) -> Vec<CredentialReference> {
    profile
        .authentication
        .references()
        .into_iter()
        .filter(|reference| {
            profile
                .credential_bindings
                .get(*reference)
                .is_some_and(|binding| binding.store == CredentialStoreKind::OsKeychain)
        })
        .cloned()
        .collect()
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveError {
    pub reference: CredentialReference,
    /// Environment variable name when the binding is environment-backed.
    pub variable: Option<String>,
    pub error: RemoteCredentialError,
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "credential `{}`", self.reference.as_str())?;
        if let Some(variable) = &self.variable {
            write!(formatter, " (environment variable `{variable}`)")?;
        }
        write!(formatter, ": {}", self.error)
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
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
    let header = authorization_header(
        &profile.authentication,
        |reference| fetch(profile, stores, reference),
        |reference, error| ResolveError {
            reference: reference.clone(),
            variable: variable_name(profile, reference),
            error: error.into(),
        },
    )?;
    Ok(header.map(|header| ProviderAuthorization { origin, header }))
}

fn fetch(
    profile: &RemoteProviderProfile,
    stores: &CredentialStores<'_>,
    reference: &CredentialReference,
) -> Result<Zeroizing<String>, ResolveError> {
    let binding = profile.credential_bindings.get(reference);
    let fail = |error| ResolveError {
        reference: reference.clone(),
        variable: binding
            .and_then(CredentialBinding::environment_variable)
            .map(str::to_string),
        error,
    };
    let binding = binding.ok_or_else(|| fail(RemoteCredentialError::Missing))?;
    let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
    let store = stores.store_for(binding.store).map_err(fail)?;
    let mut sink = HeaderSafeSink::default();
    store.with_secret(&locator, &mut sink).map_err(fail)?;
    sink.value.take().ok_or_else(|| fail(RemoteCredentialError::Missing))
}

fn variable_name(profile: &RemoteProviderProfile, reference: &CredentialReference) -> Option<String> {
    profile
        .credential_bindings
        .get(reference)
        .and_then(CredentialBinding::environment_variable)
        .map(str::to_string)
}

/// Accepts one printable-ASCII value into a zeroizing buffer.
#[derive(Default)]
struct HeaderSafeSink {
    value: Option<Zeroizing<String>>,
}

impl Sealed for HeaderSafeSink {}

impl SecretSink for HeaderSafeSink {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.value = Some(header_safe_value(bytes)?);
        Ok(())
    }
}

pub(crate) fn validate_secret(bytes: &[u8]) -> Result<(), RemoteCredentialError> {
    horizon_browser_control::remote_authorization::validate_secret(bytes).map_err(Into::into)
}

#[cfg(test)]
mod tests;
