use super::*;
use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteSessionLimits,
};
use horizon_core::remote_browser_credential::{CredentialLocator, RemoteCredentialStore, SessionCredentialStore};
use std::collections::BTreeMap;

fn capture(user: &str, key: &str, endpoint: &str) -> Account {
    let bindings = ["user", "key"]
        .map(|reference| {
            (
                CredentialReference::from(reference),
                CredentialBinding {
                    store: CredentialStoreKind::Session,
                    slot: None,
                },
            )
        })
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let profile = RemoteProviderProfile {
        adapter: RemoteAdapterKind::Browserstack,
        endpoint: ControlEndpoint::parse(endpoint).unwrap(),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings: bindings,
        limits: RemoteSessionLimits::default(),
    };
    let mut session = SessionCredentialStore::new();
    for (reference, value) in [("user", user), ("key", key)] {
        let reference = CredentialReference::from(reference);
        session
            .put(
                &CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference]),
                value.as_bytes(),
            )
            .unwrap();
    }
    Account::capture(
        &profile,
        &CredentialStores {
            session: &session,
            os_keychain: None,
            environment: None,
        },
    )
    .unwrap()
}

#[test]
fn generic_webdriver_cannot_delegate_credentials_to_native_provider_apis() {
    let profile = RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: ControlEndpoint::parse("https://hub-cloud.browserstack.com/wd/hub").unwrap(),
        authentication: RemoteAuthentication::Basic {
            username_ref: "user".into(),
            password_ref: "key".into(),
        },
        credential_bindings: BTreeMap::new(),
        limits: RemoteSessionLimits::default(),
    };
    let session = SessionCredentialStore::new();
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
        environment: None,
    };
    assert_eq!(
        Account::capture(&profile, &stores).err(),
        Some(Error::CredentialsInvalid)
    );
}

#[test]
fn separate_keys_and_users_keep_ownership_private_and_share_provider_capacity() {
    let a = capture(
        "synthetic-user",
        "synthetic-key-a",
        "https://hub-cloud.browserstack.com/wd/hub",
    );
    let same = capture(
        "synthetic-user",
        "synthetic-key-a",
        "https://hub-cloud.browserstack.com/wd/hub",
    );
    let b = capture(
        "synthetic-user",
        "synthetic-key-b",
        "https://hub-cloud.browserstack.com/wd/hub",
    );
    let alias = capture(
        "synthetic-user",
        "synthetic-key-a",
        "https://hub.browserstack.com/wd/hub",
    );
    let other = capture(
        "synthetic-other-user",
        "synthetic-key-a",
        "https://hub-cloud.browserstack.com/wd/hub",
    );
    assert_eq!(a.realm(), same.realm());
    assert_ne!(a.realm(), b.realm());
    assert_eq!(Account::capacity_namespace(), "browserstack");
    assert_ne!(a.realm(), alias.realm());
    assert_ne!(a.realm(), other.realm());
    assert!(!a.realm().contains("synthetic"));
}
