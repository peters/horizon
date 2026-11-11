use std::collections::BTreeMap;

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
};

use super::*;
use crate::secret_store::{SecretStore, UnreadableStore};

fn endpoint() -> ControlEndpoint {
    ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint")
}

fn basic_profile(username_store: CredentialStoreKind, password_store: CredentialStoreKind) -> RemoteProviderProfile {
    let mut credential_bindings = BTreeMap::new();
    credential_bindings.insert(
        CredentialReference::from("user"),
        CredentialBinding {
            store: username_store,
            slot: match username_store {
                CredentialStoreKind::File => Some("remote-browser/grid/user".to_string()),
                CredentialStoreKind::Environment => Some("REMOTE_BROWSER_USERNAME".to_string()),
                CredentialStoreKind::Session => None,
            },
        },
    );
    credential_bindings.insert(
        CredentialReference::from("key"),
        CredentialBinding {
            store: password_store,
            slot: match password_store {
                CredentialStoreKind::File => Some("remote-browser/grid/key".to_string()),
                CredentialStoreKind::Environment => Some("REMOTE_BROWSER_ACCESS_KEY".to_string()),
                CredentialStoreKind::Session => None,
            },
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
        secrets_file: None,
        environment: None,
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
        secrets_file: None,
        environment: None,
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
        secrets_file: None,
        environment: None,
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
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let session = SessionCredentialStore::new();
    let mut file = SecretStore::in_memory().expect("a store in memory");
    file.put(&locator(&profile, "user"), b"alice").expect("put user");

    let stores = CredentialStores {
        session: &session,
        secrets_file: Some(&file),
        environment: None,
    };
    let report = readiness(&profile, &stores);
    assert_eq!(report[0].state, CredentialState::Present);
    assert_eq!(report[0].store, CredentialStoreKind::File);
    assert_eq!(report[1].state, CredentialState::Missing);
    assert_eq!(report[1].store, CredentialStoreKind::Session);
    let error = resolve_authorization(&profile, &stores).expect_err("password missing");
    assert_eq!(
        (error.reference.as_str(), error.error),
        ("key", RemoteCredentialError::Missing)
    );

    let stores = CredentialStores {
        session: &session,
        secrets_file: None,
        environment: None,
    };
    assert_eq!(readiness(&profile, &stores)[0].state, CredentialState::StoreUnavailable);
    let error = resolve_authorization(&profile, &stores).expect_err("no store of this value");
    assert_eq!(error.error, RemoteCredentialError::StoreUnavailable);
}

#[test]
fn unbound_reference_is_reported_as_missing_not_a_panic() {
    let mut profile = basic_profile(CredentialStoreKind::Session, CredentialStoreKind::Session);
    profile.credential_bindings.remove(&CredentialReference::from("key"));
    let session = SessionCredentialStore::new();
    let stores = CredentialStores {
        session: &session,
        secrets_file: None,
        environment: None,
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
        secrets_file: None,
        environment: None,
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
        secrets_file: None,
        environment: None,
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
            secrets_file: None,
            environment: None,
        };
        let error = resolve_authorization(&bearer, &stores).expect_err("bad token68");
        assert_eq!(error.error, RemoteCredentialError::InvalidBearerToken, "{bad:?}");
    }
    session.put(&locator(&bearer, "key"), b"abc-._~+/=").expect("stored");
    let stores = CredentialStores {
        session: &session,
        secrets_file: None,
        environment: None,
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
fn a_store_that_cannot_be_read_is_reported_unavailable_not_missing() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let session = SessionCredentialStore::new();
    let failing = UnreadableStore::new(RemoteCredentialError::File {
        path: std::path::PathBuf::from("/home/person/.horizon/secrets.sqlite"),
        reason: "it is read-only".to_owned(),
    });
    let stores = CredentialStores {
        session: &session,
        secrets_file: Some(&failing),
        environment: None,
    };
    assert_eq!(readiness(&profile, &stores)[0].state, CredentialState::StoreUnavailable);
    let error = resolve_authorization(&profile, &stores).expect_err("a file that cannot be read");
    assert!(
        error.to_string().contains("secrets.sqlite"),
        "the refusal names the file: {error}"
    );
    assert!(
        error.to_string().starts_with("credential `user`: the secrets file"),
        "the refusal names the value and the file: {error}"
    );
}

#[test]
fn one_reference_is_one_row_whatever_its_slot_says() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut store = SecretStore::in_memory().expect("a store in memory");
    assert_eq!(store.kind(), CredentialStoreKind::File);

    // A binding without a slot is one row like any other: the file addresses a
    // value by its reference and never by a slot of the keyring.
    let mut slotless = locator(&profile, "user");
    slotless.slot = None;
    store.put(&slotless, b"alice").expect("a reference without a slot");
    let mut capture = Capture(Vec::new());
    store.with_secret(&slotless, &mut capture).expect("the value");
    assert_eq!(capture.0, b"alice");

    // Two references that name one slot are two rows, so a value of one provider
    // reference never answers another reference.
    let user = locator(&profile, "user");
    let mut aliased = locator(&profile, "key");
    aliased.slot = user.slot.clone();
    store.put(&aliased, b"bob").expect("the second reference");
    let mut capture = Capture(Vec::new());
    store.with_secret(&user, &mut capture).expect("present");
    assert_eq!(capture.0, b"alice", "the second reference wrote its own row");
    store.delete(&aliased).expect("deleted");
    assert!(store.contains(&user).expect("still there"));
    assert!(!store.contains(&aliased).expect("gone"));
}

fn wait_until(workbench: &mut CredentialWorkbench, mut done: impl FnMut(&mut CredentialWorkbench) -> bool) {
    for _ in 0..200 {
        workbench.poll();
        if done(workbench) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("workbench did not reach the expected state");
}

/// The opener of a workbench whose file holds no value, and that a test can fill.
fn memory_opener() -> StoreOpener {
    Box::new(|| {
        Ok(Box::new(SecretStore::in_memory().expect("a store in memory")) as Box<dyn RemoteCredentialStore + Send>)
    })
}

#[test]
fn workbench_probes_stores_and_deletes_file_values_off_thread() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    assert_eq!(workbench.secrets_state(), &SecretsState::Opening);
    assert_eq!(
        workbench.readiness(&profile)[0].state,
        CredentialState::Checking,
        "before the store opens the OS value is unknown"
    );
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Missing
    });
    assert!(workbench.store_handle().is_some());

    let user = CredentialReference::from("user");
    workbench
        .store_value("grid", &profile, &user, b"alice")
        .expect("store request accepted");
    assert_eq!(workbench.readiness(&profile)[0].state, CredentialState::Checking);
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Present
    });
    let notices = workbench.take_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!((notices[0].kind, &notices[0].error), (NoticeKind::StoredInFile, &None));
    assert_eq!(notices[0].provider, "grid");
    assert!(!workbench.is_busy(), "no probe or write is in flight");

    // Two references bound to one slot are one OS item: readiness agrees.
    let mut aliased = profile.clone();
    let user_slot = aliased.credential_bindings[&user].slot.clone();
    let key = CredentialReference::from("key");
    let key_binding = aliased.credential_bindings.get_mut(&key).expect("bound");
    key_binding.store = CredentialStoreKind::File;
    key_binding.slot = user_slot;
    let report = workbench.readiness(&aliased);
    assert_eq!(
        (report[0].state, report[1].state),
        (CredentialState::Present, CredentialState::Present),
        "the aliased reference reads the same cached item"
    );

    workbench
        .delete("grid", &profile, &user)
        .expect("delete request accepted");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Missing
    });
    let deleted = workbench.take_notices();
    assert_eq!(
        (deleted[0].kind, &deleted[0].error),
        (NoticeKind::DeletedFromFile, &None)
    );

    // A second provider on the same origin, reference and slot shows the
    // same profile; the answer to the first provider's save still lands on
    // the row that asked.
    workbench
        .store_value("grid", &profile, &user, b"alice")
        .expect("store request accepted");
    wait_until(&mut workbench, |w| !w.is_busy());
    let stored = workbench.take_notices();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        (
            stored[0].provider.as_str(),
            stored[0].reference.as_str(),
            stored[0].kind
        ),
        ("grid", "user", NoticeKind::StoredInFile),
        "the notice names the row that asked"
    );
    workbench
        .delete("mirror", &profile, &user)
        .expect("delete through the other provider");
    wait_until(&mut workbench, |w| !w.is_busy());
    let mirrored = workbench.take_notices();
    assert_eq!(
        (mirrored[0].provider.as_str(), mirrored[0].kind),
        ("mirror", NoticeKind::DeletedFromFile),
        "and a second provider's request answers on its own row"
    );

    assert_eq!(
        workbench.readiness(&aliased)[1].state,
        CredentialState::Missing,
        "deleting through one reference empties the aliased row too"
    );

    // Two writes queued for one OS address stay in flight until both answer.
    workbench
        .store_value("grid", &aliased, &user, b"alice")
        .expect("first write");
    workbench
        .store_value("grid", &aliased, &key, b"bob")
        .expect("second write to the same address");
    assert!(workbench.is_busy());
    wait_until(&mut workbench, |w| !w.is_busy());
    assert_eq!(
        workbench.take_notices().len(),
        2,
        "busy stays true until every queued command has answered"
    );
}

#[test]
fn workbench_reports_a_file_that_cannot_be_opened_without_values() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let path = std::path::PathBuf::from("/home/person/.horizon/secrets.sqlite");
    let reason = RemoteCredentialError::File {
        path: path.clone(),
        reason: "it is a directory".to_owned(),
    };
    let mut unavailable = CredentialWorkbench::with_opener(Box::new({
        let reason = reason.clone();
        move || Err(reason)
    }));
    wait_until(&mut unavailable, |w| {
        matches!(w.secrets_state(), SecretsState::Unavailable(_))
    });
    // The worker exits after a failed open; its disconnect must not erase the
    // specific reason it reported.
    for _ in 0..20 {
        unavailable.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(unavailable.secrets_state(), &SecretsState::Unavailable(reason.clone()));
    assert!(!unavailable.is_busy());

    // A worker that dies with probes in flight must not leave the app polling for
    // answers that will never come.
    let mut dying = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut dying, |w| w.secrets_state() == &SecretsState::Ready);
    dying.readiness(&profile);
    dying.simulate_worker_loss_for_tests();
    dying.poll();
    assert!(!dying.is_busy(), "in-flight entries settle when the worker is gone");
    assert_eq!(dying.readiness(&profile)[0].state, CredentialState::StoreUnavailable);
    assert_eq!(
        unavailable.readiness(&profile)[0].state,
        CredentialState::StoreUnavailable,
        "readiness carries the store's own reason"
    );
    assert_eq!(
        unavailable
            .store_value("grid", &profile, &CredentialReference::from("user"), b"alice")
            .expect_err("no store"),
        reason
    );
    let refused = unavailable.take_notices();
    assert_eq!(
        (refused[0].kind, &refused[0].error),
        (NoticeKind::StoredInFile, &Some(reason)),
        "a request the worker never received is still reported"
    );
    assert!(unavailable.store_handle().is_none());
}

#[test]
fn workbench_session_values_are_immediate_and_store_bound() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    let key = CredentialReference::from("key");
    workbench
        .set_session_value("grid", &profile, &key, b"s3cret")
        .expect("session value");
    assert_eq!(workbench.readiness(&profile)[1].state, CredentialState::Present);
    assert_eq!(workbench.session_value_count(), 1);
    let set = workbench.take_notices();
    assert_eq!((set[0].kind, &set[0].error), (NoticeKind::SessionValueSet, &None));
    assert_eq!(
        workbench
            .set_session_value("grid", &profile, &CredentialReference::from("user"), b"alice")
            .expect_err("user is bound to the file"),
        RemoteCredentialError::StoreUnavailable
    );
    assert_eq!(
        workbench
            .store_value("grid", &profile, &key, b"s3cret")
            .expect_err("key is bound to the session store"),
        RemoteCredentialError::StoreUnavailable
    );
    workbench.delete("grid", &profile, &key).expect("delete session value");
    assert_eq!(workbench.readiness(&profile)[1].state, CredentialState::Missing);
    workbench
        .set_session_value("grid", &profile, &key, b"again")
        .expect("session value");
    workbench.take_notices();
    workbench.clear_session();
    assert_eq!(workbench.session_value_count(), 0);
    assert!(!format!("{:?}", workbench.session_store()).contains("again"));
    let cleared = workbench.take_notices();
    assert_eq!(
        cleared.len(),
        1,
        "every reference that held a value is told it was cleared"
    );
    assert_eq!(
        (cleared[0].kind, &cleared[0].error, cleared[0].reference.as_str()),
        (NoticeKind::SessionValueCleared, &None, "key")
    );
}

#[test]
fn workbench_generation_tracks_session_and_file_mutations_without_secret_reads() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    let key = CredentialReference::from("key");
    for value in [b"original".as_slice(), b"replacement".as_slice()] {
        let before = workbench.generation();
        workbench.set_session_value("grid", &profile, &key, value).unwrap();
        assert_ne!(workbench.generation(), before);
    }
    let before = workbench.generation();
    workbench.delete("grid", &profile, &key).unwrap();
    assert_ne!(workbench.generation(), before);
    workbench.set_session_value("grid", &profile, &key, b"session").unwrap();
    let before = workbench.generation();
    workbench.clear_session();
    assert_ne!(workbench.generation(), before);
    assert_eq!(workbench.session_value_count(), 0);
    let user = CredentialReference::from("user");
    for deleting in [false, true] {
        let before = workbench.generation();
        if deleting {
            workbench.delete("grid", &profile, &user).unwrap();
        } else {
            workbench.store_value("grid", &profile, &user, b"account").unwrap();
        }
        let queued = workbench.generation();
        assert_ne!(queued, before, "invalidate before the asynchronous store mutation");
        wait_until(&mut workbench, |w| !w.is_busy());
        assert_ne!(
            workbench.generation(),
            queued,
            "discard any fetch started while the store mutation was pending"
        );
    }
}

#[test]
fn a_workbench_read_answers_one_value_to_the_caller_that_asked() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut workbench, |w| w.secrets_state() == &SecretsState::Ready);
    let user = CredentialReference::from("user");
    workbench
        .store_value("grid", &profile, &user, b"alice")
        .expect("store request accepted");
    wait_until(&mut workbench, |w| !w.is_busy());

    let mut reply = workbench
        .request_value(&profile, &user)
        .expect("the store is available");
    assert!(
        !workbench.is_busy(),
        "a read waits for no frame poll and adds no probe to the workbench"
    );
    assert_eq!(
        read_answer(&mut reply),
        Ok(b"alice".to_vec()),
        "the read copies the value the store holds"
    );
    assert!(reply.poll().is_none(), "one read answers once");

    assert_eq!(
        workbench
            .request_value(&profile, &CredentialReference::from("key"))
            .err(),
        Some(RemoteCredentialError::StoreUnavailable),
        "a reference bound to another store is not a read of the file"
    );
}

#[test]
fn a_workbench_read_names_a_missing_or_ended_store() {
    let profile = basic_profile(CredentialStoreKind::File, CredentialStoreKind::Session);
    let user = CredentialReference::from("user");

    let mut empty = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut empty, |w| w.secrets_state() == &SecretsState::Ready);
    let mut missing = empty.request_value(&profile, &user).expect("the store is available");
    assert_eq!(
        read_answer(&mut missing),
        Err(RemoteCredentialError::Missing),
        "an item the store does not hold is missing, not a value"
    );

    let reason = RemoteCredentialError::File {
        path: std::path::PathBuf::from("/home/person/.horizon/secrets.sqlite"),
        reason: "it is a directory".to_owned(),
    };
    let mut unavailable = CredentialWorkbench::with_opener(Box::new({
        let reason = reason.clone();
        move || Err(reason)
    }));
    wait_until(&mut unavailable, |w| {
        matches!(w.secrets_state(), SecretsState::Unavailable(_))
    });
    assert!(!unavailable.store_available());
    assert_eq!(
        unavailable.request_value(&profile, &user).err(),
        Some(reason),
        "a store that never opened answers with its own reason"
    );

    // A worker that ended takes no read, so the caller refuses its request instead
    // of waiting for a value that will never arrive.
    let mut dying = CredentialWorkbench::with_opener(memory_opener());
    wait_until(&mut dying, |w| w.secrets_state() == &SecretsState::Ready);
    dying.simulate_worker_loss_for_tests();
    assert_eq!(
        dying.request_value(&profile, &user).err(),
        Some(RemoteCredentialError::StoreUnavailable)
    );
}

/// The answer the workbench gives the read, once it has read the value.
fn read_answer(reply: &mut CredentialValueReply) -> Result<Vec<u8>, RemoteCredentialError> {
    for _ in 0..200 {
        if let Some(answer) = reply.poll() {
            return answer.map(|value| value.to_vec());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the workbench did not answer the read");
}

mod value_changes;
