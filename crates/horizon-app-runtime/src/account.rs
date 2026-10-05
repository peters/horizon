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
        let mut hash = Sha256::new();
        hash.update(b"horizon-native-realm-v1\0");
        hash.update(authorization.origin().as_bytes());
        hash.update(b"\0");
        hash.update(authorization.header_value().as_bytes());
        let realm = hash
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut value, byte| {
                use std::fmt::Write as _;
                let _ = write!(value, "{byte:02x}");
                value
            });
        let provider = BrowserStack::new(
            authorization.origin(),
            Zeroizing::new(authorization.header_value().to_owned()),
        )
        .map_err(|_| Error::CredentialsInvalid)?;
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
