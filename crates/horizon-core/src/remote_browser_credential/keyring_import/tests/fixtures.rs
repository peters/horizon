//! The settings, the entries and the store of one test of the import.

use std::collections::BTreeMap;
use std::sync::Arc;

use zeroize::Zeroizing;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteBrowserConfig, RemoteProviderProfile, RemoteSessionLimits,
};

use super::super::readers::KeyringEntry;
use crate::secret_store::SecretStore;

/// The slot that these settings give the value of one reference.
pub(super) const KEY_SLOT: &str = "remote-browser/grid/key";

/// A provider of one reference whose value lives in the secrets file.
pub(super) fn profile(endpoint: &str, references: &[&str]) -> RemoteProviderProfile {
    let mut credential_bindings = BTreeMap::new();
    for reference in references {
        credential_bindings.insert(
            CredentialReference::from(*reference),
            CredentialBinding {
                store: CredentialStoreKind::File,
                slot: Some(KEY_SLOT.to_string()),
            },
        );
    }
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: ControlEndpoint::parse(endpoint).expect("the endpoint"),
        authentication: authentication(references),
        credential_bindings,
        limits: RemoteSessionLimits::default(),
    }
}

/// The authentication that names every reference of a test, so the plan holds each
/// of them.
pub(super) fn authentication(references: &[&str]) -> RemoteAuthentication {
    match references {
        [one] => RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from(*one),
        },
        [user, password, ..] => RemoteAuthentication::Basic {
            username_ref: CredentialReference::from(*user),
            password_ref: CredentialReference::from(*password),
        },
        [] => RemoteAuthentication::None {},
    }
}

/// The settings of one provider with one reference.
pub(super) fn settings(endpoint: &str, references: &[&str]) -> RemoteBrowserConfig {
    let mut providers = BTreeMap::new();
    providers.insert("grid".to_string(), profile(endpoint, references));
    RemoteBrowserConfig {
        providers,
        targets: BTreeMap::new(),
    }
}

/// The account that an older Horizon gave the item of these settings.
pub(super) fn account(endpoint: &str) -> String {
    format!("{}|{KEY_SLOT}", ControlEndpoint::parse(endpoint).expect("the endpoint").origin())
}

/// One entry of the keyring of `user`.
pub(super) fn entry(user: &str, value: &str) -> KeyringEntry {
    KeyringEntry {
        user: user.to_string(),
        value: Zeroizing::new(value.as_bytes().to_vec()),
    }
}

/// A store in memory, as the secrets file of one home.
pub(super) fn store() -> Arc<SecretStore> {
    SecretStore::in_memory().expect("a secrets file in memory")
}

pub(super) fn write(store: &SecretStore, reference: &str, value: &str) {
    store
        .write(&format!("credential/{reference}"), value.as_bytes())
        .expect("the row");
}

pub(super) fn read(store: &SecretStore, reference: &str) -> Option<String> {
    store
        .read(&format!("credential/{reference}"))
        .expect("the row")
        .map(|body| String::from_utf8(body.to_vec()).expect("text"))
}

