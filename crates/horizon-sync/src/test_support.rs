//! Helpers of the tests of this crate: the values that more than one test module
//! builds, so one module does not copy another.

use std::sync::Arc;

use horizon_core::secret::{CredentialVault, SecretTarget, SecretVault};
use horizon_core::secret_store::SecretStore;

/// The secrets of a computer whose secrets file holds no value: the machine keys of
/// `keys`, and one store in memory for the credential values and one for the named
/// secrets.
pub(crate) fn keys_of_a_test(keys: horizon_core::MachineKeys) -> Arc<dyn SecretTarget> {
    use horizon_core::named_secrets::NamedSecretVault;

    let credentials = Arc::new(CredentialVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
    let named = Arc::new(NamedSecretVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    Arc::new(SecretVault::new(keys, credentials, named))
}
