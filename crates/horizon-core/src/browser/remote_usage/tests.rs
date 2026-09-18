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
