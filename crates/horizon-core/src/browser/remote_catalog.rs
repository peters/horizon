//! Account-bound discovery without credential-store or network work on the render thread.
use super::remote_usage::PreparedUsage;
use crate::remote_browser_credential::CredentialWorkbench;
pub use horizon_browser::provider_catalog::{
    CatalogCache, CatalogDevice, CatalogError, CatalogPage, CatalogQuery, target_provider,
};
use horizon_browser::{provider_catalog, remote::RemoteProviderProfile};

pub fn refresh(
    cache: &mut CatalogCache,
    name: &str,
    profile: &RemoteProviderProfile,
    credentials: &CredentialWorkbench,
) {
    if !cache.needs_refresh(name, profile) {
        return;
    }
    let prepared = PreparedUsage::new(profile, credentials).map_err(|_| CatalogError::Credentials);
    let provider = name.to_owned();
    let cloned = profile.clone();
    cache.start(name, profile, move || {
        provider_catalog::validate_provider(&cloned)?;
        let authorization = prepared?.authorization().map_err(|_| CatalogError::Credentials)?;
        provider_catalog::fetch(&provider, &cloned, authorization.header_value())
    });
}
