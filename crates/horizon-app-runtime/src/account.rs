use std::sync::Arc;

use horizon_app_provider::api::BrowserStack;
use horizon_browser::remote::RemoteProviderProfile;
use horizon_core::remote_browser_credential::{CredentialStores, resolve_authorization};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{Error, Result};

/// Captured host authorization, never serializable. A credential change creates a different realm.
pub struct Account {
    realm: String,
    provider: Arc<BrowserStack>,
}

impl Account {
    /// # Errors
    /// Uses the same configured stores as browser sessions, with no credential fallback.
    /// Store interaction follows the platform adapter; unattended reads require a noninteractive adapter.
    pub fn capture(profile: &RemoteProviderProfile, stores: &CredentialStores<'_>) -> Result<Self> {
        horizon_browser::provider_catalog::validate_provider(profile).map_err(|_| Error::CredentialsInvalid)?;
        let authorization = resolve_authorization(profile, stores)
            .map_err(|_| Error::CredentialsUnavailable)?
            .ok_or(Error::CredentialsInvalid)?;
        Self::new(authorization.origin(), authorization.header_value())
    }

    /// An authorization header that the host formed itself, for example from private credential files.
    /// # Errors
    /// The profile is not a valid provider, or the header is not a Basic authorization of the provider.
    pub fn authorized(profile: &RemoteProviderProfile, header: &str) -> Result<Self> {
        horizon_browser::provider_catalog::validate_provider(profile).map_err(|_| Error::CredentialsInvalid)?;
        Self::new(&profile.endpoint.origin(), header)
    }

    fn new(origin: &str, header: &str) -> Result<Self> {
        let mut hash = Sha256::new();
        hash.update(b"horizon-native-realm-v1\0");
        hash.update(origin.as_bytes());
        hash.update(b"\0");
        hash.update(header.as_bytes());
        let realm = hash
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut value, byte| {
                use std::fmt::Write as _;
                let _ = write!(value, "{byte:02x}");
                value
            });
        let provider =
            BrowserStack::new(origin, Zeroizing::new(header.to_owned())).map_err(|_| Error::CredentialsInvalid)?;
        Ok(Self {
            realm,
            provider: Arc::new(provider),
        })
    }

    /// Host-private state namespace. Do not return this credential fingerprint through agent interfaces.
    pub(crate) fn realm(&self) -> &str {
        &self.realm
    }

    pub(crate) fn capacity_namespace() -> &'static str {
        "browserstack"
    }

    #[must_use]
    pub fn provider(&self) -> Arc<BrowserStack> {
        Arc::clone(&self.provider)
    }
}

#[cfg(test)]
mod tests;
