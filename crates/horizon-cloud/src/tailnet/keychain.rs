use super::{Error, Result};
use keyring_core::{CredentialStore, Entry};
use std::sync::Arc;
use zeroize::Zeroizing;
const SERVICE: &str = "horizon-cloud-tailnets";
fn entry(id: &str) -> Result<Entry> {
    open()?.build(SERVICE, id, None).map_err(|_| Error::Keychain)
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
pub(super) fn read(id: &str) -> Result<Zeroizing<Vec<u8>>> {
    entry(id)?.get_secret().map(Zeroizing::new).map_err(|_| Error::Keychain)
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
#[cfg(windows)]
fn open() -> Result<Arc<CredentialStore>> {
    windows_native_keyring_store::Store::new()
        .map(|store| store as Arc<CredentialStore>)
        .map_err(|_| Error::Keychain)
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn open() -> Result<Arc<CredentialStore>> {
    Err(Error::Keychain)
}
