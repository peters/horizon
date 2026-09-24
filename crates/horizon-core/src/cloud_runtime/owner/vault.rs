use super::{Error, Result};
use keyring_core::{CredentialStore, Error as StoreError};
use std::sync::Arc;
use zeroize::Zeroizing;

const SERVICE: &str = "horizon-cloud-controller-v1";
const MAX_REGISTRATION: usize = 16 * 1024;

pub(super) trait Vault {
    fn read(&self, slot: &str) -> Result<Zeroizing<Vec<u8>>>;
    fn write(&self, slot: &str, value: &[u8]) -> Result<()>;
}

pub(super) struct NativeVault(Arc<CredentialStore>);

impl NativeVault {
    pub(super) fn open() -> Result<Self> {
        native().map(Self)
    }
}

impl Vault for NativeVault {
    fn read(&self, slot: &str) -> Result<Zeroizing<Vec<u8>>> {
        let value = Zeroizing::new(
            self.0
                .build(SERVICE, slot, None)
                .map_err(|error| map(&error))?
                .get_secret()
                .map_err(|error| map(&error))?,
        );
        if value.len() > MAX_REGISTRATION {
            return Err(Error::Registration);
        }
        Ok(value)
    }

    fn write(&self, slot: &str, value: &[u8]) -> Result<()> {
        if value.len() > MAX_REGISTRATION {
            return Err(Error::Registration);
        }
        self.0
            .build(SERVICE, slot, None)
            .map_err(|error| map(&error))?
            .set_secret(value)
            .map_err(|error| map(&error))?;
        if self.read(slot)?.as_slice() != value {
            return Err(Error::Registration);
        }
        Ok(())
    }
}

fn map(error: &StoreError) -> Error {
    match error {
        StoreError::NoEntry => Error::MissingRegistration,
        _ => Error::Store,
    }
}

#[cfg(target_os = "linux")]
fn native() -> Result<Arc<CredentialStore>> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|store| store as Arc<CredentialStore>)
        .map_err(|error| map(&error))
}
#[cfg(target_os = "macos")]
fn native() -> Result<Arc<CredentialStore>> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|store| store as Arc<CredentialStore>)
        .map_err(|error| map(&error))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn native() -> Result<Arc<CredentialStore>> {
    Err(Error::Store)
}
