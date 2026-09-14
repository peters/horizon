use std::collections::BTreeMap;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
};

use super::*;

fn endpoint() -> ControlEndpoint {
    ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint")
}

fn basic_profile(username_store: CredentialStoreKind, password_store: CredentialStoreKind) -> RemoteProviderProfile {
    let mut credential_bindings = BTreeMap::new();
    credential_bindings.insert(
        CredentialReference::from("user"),
        CredentialBinding {
            store: username_store,
            slot: (username_store == CredentialStoreKind::OsKeychain).then(|| "remote-browser/grid/user".to_string()),
        },
    );
    credential_bindings.insert(
        CredentialReference::from("key"),
        CredentialBinding {
            store: password_store,
            slot: (password_store == CredentialStoreKind::OsKeychain).then(|| "remote-browser/grid/key".to_string()),
        },
    );
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: endpoint(),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings,
        limits: RemoteSessionLimits::default(),
    }
}

fn locator(profile: &RemoteProviderProfile, reference: &str) -> CredentialLocator {
    let reference = CredentialReference::from(reference);
    CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference])
}

#[test]
fn basic_header_is_built_from_session_values_and_never_exposed() {
    let profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    let mut session = SessionCredentialStore::new();
    session.put(&locator(&profile, "user"), b"alice").expect("put user");
    session.put(&locator(&profile, "key"), b"s3cret-key").expect("put key");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let authorization = resolve_authorization(&profile, &stores)
        .expect("resolves")
        .expect("basic yields a header");
    assert_eq!(authorization.header_value(), "Basic YWxpY2U6czNjcmV0LWtleQ==");
    assert_eq!(authorization.origin(), "https://grid.example.net");
    let debug = format!("{authorization:?}");
    assert!(debug.contains("<redacted>") && !debug.contains("YWxpY2U6"), "{debug}");
    assert!(
        !format!("{session:?}").contains("s3cret"),
        "session store debug is value-free"
    );
}

#[test]
fn session_values_are_scoped_to_the_endpoint_origin() {
    let profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    let mut session = SessionCredentialStore::new();
    session.put(&locator(&profile, "user"), b"alice").expect("put user");
    session.put(&locator(&profile, "key"), b"s3cret-key").expect("put key");
    let mut other = profile.clone();
    other.endpoint = ControlEndpoint::parse("https://other.example.net/wd/hub").expect("endpoint");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let error = resolve_authorization(&other, &stores).expect_err("other origin has no values");
    assert_eq!(error.reference, CredentialReference::from("user"));
    assert_eq!(error.error, RemoteCredentialError::Missing);
    assert!(
        readiness(&other, &stores)
            .iter()
            .all(|entry| entry.state == CredentialState::Missing)
    );
}

#[test]
fn bearer_header_and_none_authentication() {
    let mut profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    profile.authentication = RemoteAuthentication::Bearer {
        token_ref: CredentialReference::from("key"),
    };
    let mut session = SessionCredentialStore::new();
    session.put(&locator(&profile, "key"), b"tok-123").expect("put token");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let authorization = resolve_authorization(&profile, &stores)
        .expect("resolves")
        .expect("bearer header");
    assert_eq!(authorization.header_value(), "Bearer tok-123");

    profile.authentication = RemoteAuthentication::None {};
    assert!(resolve_authorization(&profile, &stores).expect("resolves").is_none());
    assert!(readiness(&profile, &stores).is_empty());
}

#[test]
fn readiness_reports_missing_locked_and_unavailable_without_values() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let session = SessionCredentialStore::new();
    let mut keychain = FakeCredentialStore::new();
    keychain.put(&locator(&profile, "user"), b"alice").expect("put user");

    let stores = CredentialStores {
        session: &session,
        os_keychain: Some(&keychain),
    };
    let report = readiness(&profile, &stores);
    assert_eq!(report[0].state, CredentialState::Present);
    assert_eq!(report[0].store, CredentialStoreKind::OsKeychain);
    assert_eq!(report[1].state, CredentialState::Missing);
    assert_eq!(report[1].store, CredentialStoreKind::Session);
    let error = resolve_authorization(&profile, &stores).expect_err("password missing");
    assert_eq!(
        (error.reference.as_str(), error.error),
        ("key", RemoteCredentialError::Missing)
    );

    keychain.lock();
    let stores = CredentialStores {
        session: &session,
        os_keychain: Some(&keychain),
    };
    assert_eq!(readiness(&profile, &stores)[0].state, CredentialState::Locked);
    let error = resolve_authorization(&profile, &stores).expect_err("locked store");
    assert_eq!(error.error, RemoteCredentialError::Locked);
    assert_eq!(error.to_string(), "credential `user`: credential store is locked");

    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    assert_eq!(readiness(&profile, &stores)[0].state, CredentialState::StoreUnavailable);
    let error = resolve_authorization(&profile, &stores).expect_err("no OS store");
    assert_eq!(error.error, RemoteCredentialError::StoreUnavailable);
}

#[test]
fn unbound_reference_is_reported_as_missing_not_a_panic() {
    let mut profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    profile.credential_bindings.remove(&CredentialReference::from("key"));
    let session = SessionCredentialStore::new();
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let report = readiness(&profile, &stores);
    assert_eq!(report.len(), 2);
    assert_eq!(report[1].state, CredentialState::Missing);
}

#[test]
fn values_must_be_bounded_and_header_safe() {
    let profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    let mut session = SessionCredentialStore::new();
    assert_eq!(
        session.put(&locator(&profile, "user"), b"").expect_err("empty"),
        RemoteCredentialError::InvalidValue
    );
    assert_eq!(
        session
            .put(&locator(&profile, "user"), &vec![b'a'; MAX_SECRET_BYTES + 1])
            .expect_err("oversized"),
        RemoteCredentialError::InvalidValue
    );
    session
        .put(&locator(&profile, "user"), b"alice\r\nX-Injected: 1")
        .expect("stored as bytes");
    session.put(&locator(&profile, "key"), b"k").expect("put key");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let error = resolve_authorization(&profile, &stores).expect_err("control characters");
    assert_eq!(error.error, RemoteCredentialError::NotHeaderSafe);
}

struct Capture(Vec<u8>);

impl SecretSink for Capture {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.0 = bytes.to_vec();
        Ok(())
    }
}

#[test]
fn session_store_clears_replaces_and_deletes() {
    let profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    let mut session = SessionCredentialStore::new();
    let user = locator(&profile, "user");
    session.put(&user, b"first").expect("put");
    session.put(&user, b"second").expect("replace");
    assert_eq!(session.len(), 1);
    assert!(session.contains(&user).expect("contains"));
    session.delete(&user).expect("delete");
    assert!(!session.contains(&user).expect("contains"));
    session.delete(&user).expect("deleting twice is fine");
    session.put(&user, b"third").expect("put");
    session.clear();
    assert!(session.is_empty());
    let mut capture = Capture(Vec::new());
    assert_eq!(
        session.with_secret(&user, &mut capture).expect_err("cleared"),
        RemoteCredentialError::Missing
    );
}

#[test]
fn keyring_locator_requires_a_slot_and_maps_platform_errors() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut slotless = locator(&profile, "user");
    slotless.slot = None;
    let fake = FakeCredentialStore::new();
    assert_eq!(fake.kind(), CredentialStoreKind::OsKeychain);
    assert!(!fake.contains(&slotless).expect("fake accepts any locator"));
    assert_eq!(KEYRING_SERVICE, "horizon-remote-browser");
}
