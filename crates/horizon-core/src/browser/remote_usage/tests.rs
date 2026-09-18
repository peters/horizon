use super::*;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;

use crate::remote_browser_credential::FakeCredentialStore;
use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, RemoteAuthentication, RemoteSessionLimits,
};

const PLAN: &str = r#"{"parallel_sessions_running":3,"parallel_sessions_max_allowed":10,"team_parallel_sessions_max_allowed":5,"queued_sessions":2}"#;

fn profile() -> RemoteProviderProfile {
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Browserstack,
        endpoint: ControlEndpoint::parse("https://hub-cloud.browserstack.com/wd/hub").expect("endpoint"),
        authentication: RemoteAuthentication::Bearer {
            token_ref: CredentialReference::from("key"),
        },
        credential_bindings: [(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::Session,
                slot: None,
            },
        )]
        .into(),
        limits: RemoteSessionLimits::default(),
    }
}

fn credentials() -> CredentialWorkbench {
    CredentialWorkbench::with_opener(Box::new(|| Ok(Box::new(FakeCredentialStore::new()))))
}

#[test]
fn normalizes_capacity_and_rejects_missing_or_negative_counts() {
    let adapter = UsageAdapter::Browserstack;
    assert_eq!(
        adapter.decode(PLAN.as_bytes()).expect("plan"),
        ProviderUsage {
            running: 3,
            allowed: 5,
            queued: 2
        }
    );
    assert_eq!(
        adapter
            .decode(br#"{"parallel_sessions_running":0,"parallel_sessions_max_allowed":8,"queued_sessions":0}"#)
            .expect("no team allocation")
            .allowed,
        8
    );
    for body in [
        "{}",
        "not json",
        r#"{"parallel_sessions_running":-1,"parallel_sessions_max_allowed":8,"queued_sessions":0}"#,
    ] {
        assert_eq!(adapter.decode(body.as_bytes()), Err(UsageError::InvalidResponse));
    }
}

#[test]
fn transport_reports_access_failures_without_exposing_response_or_following_redirects() {
    for (status, body, expected) in [
        (
            "200 OK",
            PLAN,
            Ok(ProviderUsage {
                running: 3,
                allowed: 5,
                queued: 2,
            }),
        ),
        (
            "401 Unauthorized",
            "private provider error",
            Err(UsageError::AccessDenied),
        ),
        ("403 Forbidden", "private provider error", Err(UsageError::AccessDenied)),
        ("302 Found", "", Err(UsageError::Unavailable)),
        (
            "429 Too Many Requests",
            "private provider error",
            Err(UsageError::Unavailable),
        ),
        ("200 OK", "{}", Err(UsageError::InvalidResponse)),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
            let mut request = vec![];
            loop {
                let mut bytes = [0; 1024];
                let count = stream.read(&mut bytes).expect("request");
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).expect("request text").to_ascii_lowercase();
            assert!(request.contains("authorization: bearer synthetic-key"));
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/never\r\nConnection: close\r\n\r\n{body}", body.len()).expect("response");
        });
        assert_eq!(
            fetch_usage(
                UsageAdapter::Browserstack,
                &format!("http://{address}/plan"),
                "Bearer synthetic-key"
            ),
            expected
        );
        server.join().expect("server");
    }
}

#[test]
fn errors_keep_the_last_sample_and_disconnected_workers_recover() {
    let (sender, receiver) = channel();
    let usage = ProviderUsage {
        running: 2,
        allowed: 4,
        queued: 0,
    };
    let mut monitor = ProviderUsageMonitor {
        pending: Some(receiver),
        ..ProviderUsageMonitor::default()
    };
    let fetched_at = Instant::now()
        .checked_sub(Duration::from_secs(120))
        .expect("earlier fetch");
    sender.send(Ok((usage, fetched_at))).expect("success");
    monitor.poll();
    let sample = monitor.sample.expect("sample");
    assert_eq!(sample.1, fetched_at, "hidden views must retain the actual fetch time");
    let (sender, receiver) = channel();
    monitor.pending = Some(receiver);
    sender.send(Err(UsageError::AccessDenied)).expect("failure");
    monitor.poll();
    assert_eq!(monitor.sample, Some(sample));
    assert_eq!(monitor.error, Some(UsageError::AccessDenied));
    let (sender, receiver) = channel();
    monitor.pending = Some(receiver);
    drop(sender);
    monitor.poll();
    assert_eq!(monitor.error, Some(UsageError::Unavailable));
    assert!(!monitor.refreshing());
}

#[test]
fn profile_changes_discard_old_results_and_providers_have_independent_state() {
    let original = profile();
    let mut replacement = original.clone();
    replacement.endpoint = ControlEndpoint::parse("https://other.example.test").expect("endpoint");
    let usage = ProviderUsage {
        running: 2,
        allowed: 4,
        queued: 0,
    };
    let (sender, receiver) = channel();
    sender.send(Ok((usage, Instant::now()))).expect("old response");
    let mut monitor = ProviderUsageMonitor {
        profile: Some(original),
        pending: Some(receiver),
        sample: Some((usage, Instant::now())),
        ..ProviderUsageMonitor::default()
    };
    monitor.update(&replacement, &credentials(), false);
    assert!(monitor.sample.is_none());
    assert_eq!(monitor.error, Some(UsageError::Credentials));
    let other = ProviderUsageMonitor::default();
    assert!(other.sample.is_none());
    assert!(other.error.is_none());
}

#[test]
fn a_pending_refresh_is_not_duplicated_and_success_is_rate_limited() {
    let profile = profile();
    let (_sender, receiver) = channel();
    let mut monitor = ProviderUsageMonitor {
        profile: Some(profile.clone()),
        pending: Some(receiver),
        ..ProviderUsageMonitor::default()
    };
    monitor.update(&profile, &credentials(), true);
    assert!(monitor.refreshing());
    assert!(monitor.last_attempt.is_none());
    monitor.pending = None;
    monitor.last_attempt = Some(Instant::now());
    monitor.update(&profile, &credentials(), false);
    assert!(monitor.error.is_none());
    monitor.update(&profile, &credentials(), true);
    assert_eq!(monitor.error, Some(UsageError::Credentials));
}

#[test]
fn memory_snapshot_survives_credential_clear_and_unsupported_providers_do_not_fetch() {
    let mut profile = profile();
    let mut credentials = credentials();
    credentials
        .set_session_value("cloud", &profile, &CredentialReference::from("key"), b"synthetic-key")
        .expect("credential");
    let prepared = PreparedUsage::new(&profile, &credentials).expect("snapshot");
    credentials.clear_session();
    let auth = resolve_authorization(
        &prepared.profile,
        &CredentialStores {
            session: &prepared.memory,
            os_keychain: None,
            environment: None,
        },
    )
    .expect("snapshot auth")
    .expect("header");
    assert_eq!(auth.header_value(), "Bearer synthetic-key");
    assert!(PreparedUsage::new(&profile, &credentials).is_err());
    profile.adapter = RemoteAdapterKind::Webdriver;
    assert!(!ProviderUsageMonitor::supported(&profile));
    let mut monitor = ProviderUsageMonitor::default();
    monitor.update(&profile, &credentials, true);
    assert_eq!(monitor.error, Some(UsageError::Unsupported));
    assert!(!monitor.refreshing());
}

#[test]
fn session_credentials_do_not_wait_for_an_unrelated_keychain_lock() {
    let profile = profile();
    let mut credentials = credentials();
    credentials
        .set_session_value("cloud", &profile, &CredentialReference::from("key"), b"synthetic-key")
        .expect("credential");
    let deadline = Instant::now() + Duration::from_secs(2);
    let keychain = loop {
        credentials.poll();
        if let Some(store) = credentials.keychain_store() {
            break store;
        }
        assert!(Instant::now() < deadline, "fake keychain did not open");
        std::thread::sleep(Duration::from_millis(1));
    };
    let guard = keychain.lock().expect("hold unrelated OS operation");
    let prepared = PreparedUsage::new(&profile, &credentials).expect("snapshot");
    let (sender, receiver) = channel();
    let worker = std::thread::spawn(move || {
        sender.send(prepared.authorization().is_ok()).expect("answer");
    });
    let resolved_while_locked = receiver.recv_timeout(Duration::from_secs(1));
    drop(guard);
    worker.join().expect("worker");
    assert_eq!(resolved_while_locked, Ok(true));
}

#[test]
fn two_accounts_on_one_provider_keep_distinct_bound_credentials() {
    let mut first = profile();
    let mut second = profile();
    let mut credentials = credentials();
    for (profile, prefix) in [(&mut first, "first"), (&mut second, "second")] {
        let user = CredentialReference::from(format!("{prefix}-user").as_str());
        let key = CredentialReference::from(format!("{prefix}-key").as_str());
        profile.authentication = RemoteAuthentication::Basic {
            username_ref: user.clone(),
            password_ref: key.clone(),
        };
        profile.credential_bindings = [user.clone(), key.clone()]
            .into_iter()
            .map(|reference| {
                (
                    reference,
                    CredentialBinding {
                        store: CredentialStoreKind::Session,
                        slot: None,
                    },
                )
            })
            .collect();
        credentials
            .set_session_value(prefix, profile, &user, prefix.as_bytes())
            .expect("user");
        credentials
            .set_session_value(prefix, profile, &key, b"synthetic-key")
            .expect("key");
    }
    let first_request = PreparedUsage::new(&first, &credentials).expect("first account");
    let second_request = PreparedUsage::new(&second, &credentials).expect("second account");
    let first_auth = first_request.authorization().expect("first authorization");
    let second_auth = second_request.authorization().expect("second authorization");
    assert_eq!(first_auth.origin(), second_auth.origin());
    assert_eq!(first_auth.header_value(), "Basic Zmlyc3Q6c3ludGhldGljLWtleQ==");
    assert_eq!(second_auth.header_value(), "Basic c2Vjb25kOnN5bnRoZXRpYy1rZXk=");
    credentials
        .set_session_value(
            "first",
            &first,
            &CredentialReference::from("first-key"),
            b"replacement-key",
        )
        .expect("rotate first account");
    let unchanged = PreparedUsage::new(&second, &credentials)
        .expect("second still ready")
        .authorization()
        .expect("second auth");
    assert_eq!(unchanged.header_value(), second_auth.header_value());
    let changed = PreparedUsage::new(&first, &credentials)
        .expect("first still ready")
        .authorization()
        .expect("first auth");
    assert_ne!(changed.header_value(), first_auth.header_value());
}

#[test]
fn usage_delegation_rejects_unrelated_and_lookalike_origins() {
    let adapter = UsageAdapter::Browserstack;
    assert!(adapter.authorizes_origin("https://hub-cloud.browserstack.com"));
    for origin in [
        "https://grid.example.test",
        "https://hub-cloud.browserstack.com.evil.test",
        "http://hub-cloud.browserstack.com",
        "https://hub-cloud.browserstack.com:8443",
    ] {
        assert!(!adapter.authorizes_origin(origin));
        let mut profile = profile();
        if let Ok(endpoint) = ControlEndpoint::parse(origin) {
            profile.endpoint = endpoint;
            let mut credentials = credentials();
            credentials
                .set_session_value("cloud", &profile, &CredentialReference::from("key"), b"synthetic-key")
                .expect("credential");
            assert_eq!(
                PreparedUsage::new(&profile, &credentials)
                    .expect("snapshot")
                    .fetch(adapter),
                Err(UsageError::Unsupported)
            );
        }
    }
}

#[test]
fn stalled_workers_settle_with_an_error_and_keep_the_last_sample() {
    let (_sender, receiver) = channel();
    let sample = (
        ProviderUsage {
            running: 1,
            allowed: 4,
            queued: 0,
        },
        Instant::now(),
    );
    let mut monitor = ProviderUsageMonitor {
        pending: Some(receiver),
        sample: Some(sample),
        last_attempt: Instant::now().checked_sub(REQUEST_TIMEOUT),
        ..ProviderUsageMonitor::default()
    };
    monitor.poll();
    assert!(!monitor.refreshing());
    assert_eq!(monitor.error, Some(UsageError::Unavailable));
    assert_eq!(monitor.sample, Some(sample));
}

#[test]
fn stalled_usage_keychain_does_not_hold_the_allocation_store() {
    let mut profile = profile();
    let binding = profile
        .credential_bindings
        .get_mut(&CredentialReference::from("key"))
        .expect("binding");
    binding.store = CredentialStoreKind::OsKeychain;
    binding.slot = Some("test-slot".into());
    let mut credentials = credentials();
    let deadline = Instant::now() + Duration::from_secs(2);
    let shared = loop {
        credentials.poll();
        if let Some(shared) = credentials.keychain_store() {
            break shared;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let mut prepared = PreparedUsage::new(&profile, &credentials).expect("prepare independent reader");
    let (entered, reading) = channel();
    let (release, resume) = channel();
    prepared.keychain = Some(Box::new(move || {
        entered.send(()).expect("reading");
        resume.recv().expect("release");
        Ok(Box::new(FakeCredentialStore::new()))
    }));
    let worker = std::thread::spawn(move || prepared.authorization());
    reading.recv_timeout(Duration::from_secs(1)).expect("reader started");
    assert!(shared.try_lock().is_ok(), "usage must never retain allocation's mutex");
    release.send(()).expect("release reader");
    assert!(worker.join().expect("worker").is_err());
}

#[test]
fn worker_capacity_is_bounded_and_recovers_after_a_worker_exits() {
    let counter = AtomicUsize::new(0);
    let mut permits: Vec<_> = (0..MAX_USAGE_WORKERS)
        .map(|_| UsageWorkerPermit::acquire(&counter).expect("capacity"))
        .collect();
    assert!(UsageWorkerPermit::acquire(&counter).is_none());
    drop(permits.pop());
    let recovered = UsageWorkerPermit::acquire(&counter).expect("released slot");
    drop(permits);
    assert_eq!(counter.load(Ordering::Relaxed), 1);
    drop(recovered);
    assert_eq!(counter.load(Ordering::Relaxed), 0);
}

#[test]
fn duplicate_environment_references_resolve_from_the_original_binding() {
    if std::env::var_os("HORIZON_USAGE_DUPLICATE_TEST").is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "browser::remote_usage::tests::duplicate_environment_references_resolve_from_the_original_binding",
            ])
            .env("HORIZON_USAGE_DUPLICATE_TEST", "synthetic")
            .status()
            .expect("isolated environment test");
        assert!(status.success());
        return;
    }
    let mut profile = profile();
    profile.authentication = RemoteAuthentication::Basic {
        username_ref: CredentialReference::from("key"),
        password_ref: CredentialReference::from("key"),
    };
    let binding = profile
        .credential_bindings
        .get_mut(&CredentialReference::from("key"))
        .expect("binding");
    binding.store = CredentialStoreKind::Environment;
    binding.slot = Some("HORIZON_USAGE_DUPLICATE_TEST".into());
    let mut credentials = credentials();
    credentials.load_environment_bindings(&horizon_browser::remote::RemoteBrowserConfig {
        providers: [("cloud".into(), profile.clone())].into(),
        ..Default::default()
    });
    let prepared =
        PreparedUsage::new(&profile, &credentials).expect("both references read the original environment binding");
    assert!(prepared.authorization().is_ok());
}
