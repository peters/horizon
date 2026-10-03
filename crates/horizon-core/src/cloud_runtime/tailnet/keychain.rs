// Reads remain private to the deployment adapter; no caller-supplied sink.
use horizon_cloud::tailnet::{Error, KEYCHAIN_SERVICE, Result};
use keyring_core::CredentialStore;
use std::sync::Arc;
use zeroize::Zeroizing;

pub(super) fn read(id: &str) -> Result<Zeroizing<Vec<u8>>> {
    open()?
        .build(KEYCHAIN_SERVICE, id, None)
        .and_then(|entry| entry.get_secret())
        .map(Zeroizing::new)
        .map_err(|_| Error::Keychain)
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the task-owned isolated Secret Service fixture"]
    fn private_enrollment_reads_saved_and_replaced_keys() {
        let root = std::env::var_os("HORIZON_TAILNET_TEST_ROOT").expect("private fixture required");
        let root = std::path::PathBuf::from(root);
        let fixture = root.parent().unwrap();
        assert!(fixture.starts_with("/var/tmp/h1166"));
        assert!(
            fixture
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("native-security-")
        );
        assert_eq!(
            std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
            std::env::var("HORIZON_TAILNET_TEST_BUS").expect("private bus required")
        );
        let store = horizon_cloud::tailnet::Store::new(root);
        let first = "tskey-auth-synthetic12345678901234567890";
        let second = "tskey-auth-synthetic98765432109876543210";
        let catalog = store.save(None, "Synthetic enrollment", first).unwrap();
        let id = &catalog.tailnets[0].id;
        let original_slot = store.credential_slot(id).unwrap();
        assert_eq!(
            read(&store.credential_slot(id).unwrap()).unwrap().as_slice(),
            first.as_bytes()
        );
        store.save(Some(id), "Synthetic replacement", second).unwrap();
        assert_eq!(
            read(&store.credential_slot(id).unwrap()).unwrap().as_slice(),
            second.as_bytes()
        );
        let slot = store.credential_slot(id).unwrap();
        assert_ne!(original_slot, slot);
        assert!(read(&original_slot).is_err());
        store.delete(id).unwrap();
        assert!(read(&slot).is_err());
        assert!(store.load().unwrap().tailnets.is_empty());
    }
}
