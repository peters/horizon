use std::collections::BTreeMap;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
};

use std::sync::Arc;

use super::keyring_store::map_error;
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

impl super::Sealed for Capture {}

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
fn basic_usernames_may_not_contain_colons_and_bearer_tokens_follow_token68() {
    let profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    let mut session = SessionCredentialStore::new();
    session.put(&locator(&profile, "user"), b"alice:admin").expect("stored");
    session.put(&locator(&profile, "key"), b"k").expect("stored");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    let error = resolve_authorization(&profile, &stores).expect_err("colon in username");
    assert_eq!(error.error, RemoteCredentialError::InvalidUsername);
    assert_eq!(error.reference.as_str(), "user");

    let mut bearer = profile.clone();
    bearer.authentication = RemoteAuthentication::Bearer {
        token_ref: CredentialReference::from("key"),
    };
    for bad in [&b"tok en"[..], b"tok:en", b"tok\"en", b"=", b"==abc"] {
        session.put(&locator(&bearer, "key"), bad).expect("stored");
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
        };
        let error = resolve_authorization(&bearer, &stores).expect_err("bad token68");
        assert_eq!(error.error, RemoteCredentialError::InvalidBearerToken, "{bad:?}");
    }
    session.put(&locator(&bearer, "key"), b"abc-._~+/=").expect("stored");
    let stores = CredentialStores {
        session: &session,
        os_keychain: None,
    };
    assert_eq!(
        resolve_authorization(&bearer, &stores)
            .expect("ok")
            .expect("header")
            .header_value(),
        "Bearer abc-._~+/="
    );
}

#[test]
fn a_failing_store_is_reported_unavailable_not_missing() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let session = SessionCredentialStore::new();
    let failing = KeyringCredentialStore::with_store(Arc::new(MockStore::failing()));
    let stores = CredentialStores {
        session: &session,
        os_keychain: Some(&failing),
    };
    assert_eq!(readiness(&profile, &stores)[0].state, CredentialState::StoreUnavailable);
    assert_eq!(
        resolve_authorization(&profile, &stores)
            .expect_err("platform failure")
            .error,
        RemoteCredentialError::Platform {
            kind: "platform_failure"
        }
    );
}

#[test]
fn keyring_errors_map_to_value_free_states() {
    use keyring_core::Error;
    let boxed = || Box::new(std::io::Error::other("platform detail")) as Box<dyn std::error::Error + Send + Sync>;
    assert_eq!(map_error(&Error::NoEntry), RemoteCredentialError::Missing);
    assert_eq!(
        map_error(&Error::NoStorageAccess(boxed())),
        RemoteCredentialError::Locked
    );
    assert_eq!(
        map_error(&Error::NoDefaultStore),
        RemoteCredentialError::StoreUnavailable
    );
    assert_eq!(
        map_error(&Error::NotSupportedByStore("x".into())),
        RemoteCredentialError::StoreUnavailable
    );
    assert_eq!(
        map_error(&Error::TooLong("user".into(), 1)),
        RemoteCredentialError::InvalidValue
    );
    assert_eq!(
        map_error(&Error::Invalid("a".into(), "b".into())),
        RemoteCredentialError::InvalidValue
    );
    assert_eq!(
        map_error(&Error::PlatformFailure(boxed())),
        RemoteCredentialError::Platform {
            kind: "platform_failure"
        }
    );
    assert!(!format!("{}", map_error(&Error::PlatformFailure(boxed()))).contains("platform detail"));
}

#[test]
fn keyring_adapter_round_trips_through_a_mock_store_and_requires_a_slot() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mock = Arc::new(MockStore::default());
    let mut store = KeyringCredentialStore::with_store(mock.clone());
    let user = locator(&profile, "user");
    assert!(!store.contains(&user).expect("absent"));
    store.put(&user, b"alice").expect("stored");
    assert!(store.contains(&user).expect("present"));
    let mut prefix = user.clone();
    prefix.slot = Some("remote-browser/grid/use".into());
    assert!(
        !store.contains(&prefix).expect("prefix slot absent"),
        "a loose search match must not count as presence"
    );
    assert_eq!(mock.reads(), 0, "presence probes never read a value");
    let keys = mock.keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, KEYRING_SERVICE);
    assert_eq!(keys[0].1, "https://grid.example.net|remote-browser/grid/user");
    let mut capture = Capture(Vec::new());
    store.with_secret(&user, &mut capture).expect("copied into the sink");
    assert_eq!(capture.0, b"alice");
    assert_eq!(mock.reads(), 1);
    store.delete(&user).expect("deleted");
    assert!(!store.contains(&user).expect("absent again"));
    store.delete(&user).expect("deleting twice is fine");

    let mut slotless = user.clone();
    slotless.slot = None;
    assert_eq!(
        store.put(&slotless, b"x").expect_err("slot required"),
        RemoteCredentialError::Missing
    );
    assert_eq!(
        store.contains(&slotless).expect_err("slot required"),
        RemoteCredentialError::Missing
    );
}

type MockItems = Arc<std::sync::Mutex<std::collections::HashMap<(String, String), Vec<u8>>>>;

/// In-memory keyring-core store: exercises the adapter without any platform.
/// Counts value reads so tests can prove which operations never read one.
#[derive(Default)]
struct MockStore {
    items: MockItems,
    reads: Arc<std::sync::atomic::AtomicUsize>,
    failing: bool,
}

impl MockStore {
    fn failing() -> Self {
        Self {
            failing: true,
            ..Self::default()
        }
    }

    fn keys(&self) -> Vec<(String, String)> {
        self.items.lock().expect("lock").keys().cloned().collect()
    }

    fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn platform_down() -> keyring_core::Error {
        keyring_core::Error::PlatformFailure(Box::new(std::io::Error::other("down")))
    }
}

impl std::fmt::Debug for MockStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MockStore")
    }
}

impl keyring_core::api::CredentialStoreApi for MockStore {
    fn vendor(&self) -> String {
        "horizon-test".into()
    }

    fn id(&self) -> String {
        "mock".into()
    }

    fn build(
        &self,
        service: &str,
        user: &str,
        _modifiers: Option<&std::collections::HashMap<&str, &str>>,
    ) -> keyring_core::Result<keyring_core::Entry> {
        if self.failing {
            return Err(Self::platform_down());
        }
        Ok(keyring_core::Entry::new_with_credential(Arc::new(MockCredential {
            store: Arc::clone(&self.items),
            reads: Arc::clone(&self.reads),
            key: (service.to_string(), user.to_string()),
        })))
    }

    /// Matches loosely on purpose, like the platform searches may: the
    /// adapter must compare specifiers exactly.
    fn search(&self, spec: &std::collections::HashMap<&str, &str>) -> keyring_core::Result<Vec<keyring_core::Entry>> {
        if self.failing {
            return Err(Self::platform_down());
        }
        let service = spec.get("service").copied().unwrap_or_default();
        let user = spec.get("user").copied().unwrap_or_default();
        Ok(self
            .keys()
            .into_iter()
            .filter(|key| key.0.contains(service) && key.1.contains(user))
            .map(|key| {
                keyring_core::Entry::new_with_credential(Arc::new(MockCredential {
                    store: Arc::clone(&self.items),
                    reads: Arc::clone(&self.reads),
                    key,
                }))
            })
            .collect())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct MockCredential {
    store: MockItems,
    reads: Arc<std::sync::atomic::AtomicUsize>,
    key: (String, String),
}

impl std::fmt::Debug for MockCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MockCredential")
    }
}

impl keyring_core::api::CredentialApi for MockCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring_core::Result<()> {
        self.store
            .lock()
            .expect("lock")
            .insert(self.key.clone(), secret.to_vec());
        Ok(())
    }

    fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.store
            .lock()
            .expect("lock")
            .get(&self.key)
            .cloned()
            .ok_or(keyring_core::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring_core::Result<()> {
        self.store
            .lock()
            .expect("lock")
            .remove(&self.key)
            .map(|_| ())
            .ok_or(keyring_core::Error::NoEntry)
    }

    fn get_credential(&self) -> keyring_core::Result<Option<Arc<keyring_core::Credential>>> {
        if self.store.lock().expect("lock").contains_key(&self.key) {
            Ok(None)
        } else {
            Err(keyring_core::Error::NoEntry)
        }
    }

    fn get_specifiers(&self) -> Option<(String, String)> {
        Some(self.key.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn fake_store_addresses_items_like_the_os_adapter() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut slotless = locator(&profile, "user");
    slotless.slot = None;
    let mut fake = FakeCredentialStore::new();
    assert_eq!(fake.kind(), CredentialStoreKind::OsKeychain);
    assert_eq!(
        fake.contains(&slotless).expect_err("slot required"),
        RemoteCredentialError::Missing
    );
    assert_eq!(
        fake.put(&slotless, b"x").expect_err("slot required"),
        RemoteCredentialError::Missing
    );

    // Two references bound to one slot are one OS item: the later write wins.
    let user = locator(&profile, "user");
    let mut aliased = locator(&profile, "key");
    aliased.slot = user.slot.clone();
    fake.put(&user, b"alice").expect("stored");
    fake.put(&aliased, b"bob").expect("replaced through the alias");
    let mut capture = Capture(Vec::new());
    fake.with_secret(&user, &mut capture).expect("present");
    assert_eq!(capture.0, b"bob");
    fake.delete(&aliased).expect("deleted through the alias");
    assert!(!fake.contains(&user).expect("gone for both references"));
    assert_eq!(KEYRING_SERVICE, "horizon-remote-browser");
}

/// Opt-in smoke against this computer's real OS store; it creates and then
/// deletes one item under a unique slot. Run with `--ignored` on a desktop
/// session where the store is unlocked.
#[test]
#[ignore = "touches the real OS credential store"]
fn os_store_round_trip_smoke() {
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut store = KeyringCredentialStore::open().expect("OS store available");
    let mut item = locator(&profile, "user");
    item.slot = Some(format!("remote-browser/smoke/{}", std::process::id()));
    assert!(!store.contains(&item).expect("probe before put"));
    store.put(&item, b"smoke-value").expect("put");
    assert!(store.contains(&item).expect("probe after put"));
    let stores = CredentialStores {
        session: &SessionCredentialStore::new(),
        os_keychain: Some(&store),
    };
    let mut smoke = profile.clone();
    smoke
        .credential_bindings
        .get_mut(&CredentialReference::from("user"))
        .expect("bound")
        .slot = item.slot.clone();
    assert_eq!(readiness(&smoke, &stores)[0].state, CredentialState::Present);
    let mut capture = Capture(Vec::new());
    store.with_secret(&item, &mut capture).expect("read back");
    assert_eq!(capture.0, b"smoke-value");
    store.delete(&item).expect("delete");
    assert!(!store.contains(&item).expect("probe after delete"));
}
