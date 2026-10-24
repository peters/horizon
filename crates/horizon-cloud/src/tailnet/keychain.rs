use super::{API_KEYCHAIN_SERVICE, KEYCHAIN_SERVICE};
use super::{Error, Result};
use keyring_core::{CredentialStore, Entry};
use std::sync::Arc;
fn entry(id: &str) -> Result<Entry> {
    entry_for(KEYCHAIN_SERVICE, id)
}
fn entry_for(service: &str, id: &str) -> Result<Entry> {
    open()?.build(service, id, None).map_err(|_| Error::Keychain)
}
pub(super) fn api_read(id: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
    let entry = entry_for(API_KEYCHAIN_SERVICE, id)?;
    match entry.get_secret() {
        Ok(bytes) => Ok(Some(zeroize::Zeroizing::new(bytes))),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(_) => Err(Error::Keychain),
    }
}
pub(super) fn api_put(id: &str, bytes: &[u8]) -> Result<()> {
    entry_for(API_KEYCHAIN_SERVICE, id)?
        .set_secret(bytes)
        .map_err(|_| Error::Keychain)
}
pub(super) fn api_delete(id: &str) -> Result<()> {
    let entry = entry_for(API_KEYCHAIN_SERVICE, id)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(_) => Err(Error::Keychain),
    }
}
pub(super) fn put(id: &str, key: &str) -> Result<()> {
    entry(id)?.set_secret(key.as_bytes()).map_err(|_| Error::Keychain)
}
pub(super) fn delete(id: &str) -> Result<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(_) => Err(Error::Keychain),
    }
}
#[cfg(target_os = "linux")]
fn open() -> Result<Arc<CredentialStore>> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|store| store as Arc<CredentialStore>)
        .map_err(|_| Error::Keychain)
}
#[cfg(target_os = "macos")]
fn open() -> Result<Arc<CredentialStore>> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|store| store as Arc<CredentialStore>)
        .map_err(|_| Error::Keychain)
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn open() -> Result<Arc<CredentialStore>> {
    Err(Error::Keychain)
}
