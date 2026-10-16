use super::super::{Scope, scope, socks::Gate as _};
use super::*;
use std::{net::Ipv4Addr, sync::atomic::AtomicUsize};

#[cfg(target_os = "linux")]
mod real_kernel;
mod relay;

fn binding(address: SocketAddr, peer: &str) -> Binding {
    Binding::Tailnet {
        desktop: "desktop-fixture".into(),
        peer: peer.into(),
        name: "fixture.example.ts.net".into(),
        address,
    }
}

fn prepared(address: SocketAddr) -> Prepared {
    Prepared {
        attempt: Attempt::new(),
        endpoint: Endpoint::new("fixture.example.ts.net", address.port()).unwrap(),
        source: Source::fixture(move |_| Ok(binding(address, "peer-fixture"))),
        binding: binding(address, "peer-fixture"),
    }
}

#[test]
fn malformed_inputs_do_not_start_a_source_lookup() {
    for host in [
        "",
        ".",
        "printer..",
        "-printer.local",
        "printer-.local",
        "https://printer.local",
        "printer\n.local",
        "::1",
    ] {
        assert_eq!(Endpoint::new(host, 80), Err(Error::InvalidEndpoint), "{host:?}");
    }
    assert_eq!(Endpoint::new("printer.local", 0), Err(Error::InvalidEndpoint));
    assert_eq!(Endpoint::new(&"x".repeat(254), 80), Err(Error::InvalidEndpoint));
    assert_eq!(Endpoint::new(&"x".repeat(1024 * 1024), 80), Err(Error::InvalidEndpoint));
    assert_eq!(Endpoint::new("Printer.Local.", 80).unwrap().host, "printer.local");
}

#[test]
fn another_name_address_or_port_is_refused_before_source_observation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let address = "127.0.0.1:8080".parse().unwrap();
    let mut prepared = prepared(address);
    prepared.source = Source::fixture(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
        Ok(binding(address, "peer-fixture"))
    });
    let gate = Gate {
        prepared,
        reason: Mutex::new(None),
    };
    for request in [
        Destination::Name("secret.example.ts.net".into(), 8080),
        Destination::Name("fixture.example.ts.net".into(), 8081),
        Destination::Address("127.0.0.2:8080".parse().unwrap()),
    ] {
        assert!(gate.admit(&request).is_err());
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "unapproved names cause no DNS/client lookup"
    );
    assert!(gate.admit(&Destination::Address(address)).is_ok());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn peer_replacement_at_the_same_address_requires_new_admission() {
    let address = "127.0.0.1:8080".parse().unwrap();
    let mut prepared = prepared(address);
    prepared.source = Source::fixture(move |_| Ok(binding(address, "replacement-peer")));
    assert!(matches!(Gateway::start(prepared), Err(Error::BindingChanged)));
}

#[test]
fn busy_observation_refuses_one_request_without_reopening_or_revoking() {
    let address = "127.0.0.1:8080".parse().unwrap();
    let mut prepared = prepared(address);
    prepared.source = Source::fixture(|_| Err(Error::Busy));
    let gate = Gate {
        prepared,
        reason: Mutex::new(None),
    };
    assert!(gate.admit(&Destination::Address(address)).is_err());
    assert_eq!(gate.reason(), None);
}

#[test]
fn revocation_during_a_successful_source_observation_refuses_the_admission() {
    let address = "127.0.0.1:8080".parse().unwrap();
    let (observing, entered) = std::sync::mpsc::channel();
    let (resume, continuing) = std::sync::mpsc::channel();
    let continuing = Mutex::new(continuing);
    let mut prepared = prepared(address);
    prepared.source = Source::fixture(move |_| {
        observing.send(()).map_err(|_| Error::SourceUnavailable)?;
        continuing
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| Error::SourceUnavailable)?;
        Ok(binding(address, "peer-fixture"))
    });
    let gate = Arc::new(Gate {
        prepared,
        reason: Mutex::new(None),
    });
    let admitting = Arc::clone(&gate);
    let admission = thread::spawn(move || admitting.admit(&Destination::Address(address)));
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    gate.revoke(Reason::BindingChanged);
    resume.send(()).unwrap();
    assert_eq!(
        admission.join().unwrap(),
        Err(horizon_cloud_protocol::local_network::Reply::NotAllowed)
    );
    assert_eq!(gate.reason(), Some(Reason::BindingChanged));
    assert!(gate.admit(&Destination::Address(address)).is_err());
}

#[test]
fn a_source_error_is_fixed_and_never_publishes_client_account_or_peer_values() {
    let address = "127.0.0.1:8080".parse().unwrap();
    let mut prepared = prepared(address);
    prepared.source = Source::fixture(|_| Err(Error::SourceUnavailable));
    let gate = Gate {
        prepared,
        reason: Mutex::new(None),
    };
    assert!(gate.check().is_err());
    assert_eq!(gate.reason(), Some(Reason::SourceUnavailable));
    assert_eq!(
        Error::SourceUnavailable.to_string(),
        "The endpoint source is unavailable"
    );
}

fn lan_scope(answers: Arc<Mutex<Vec<SocketAddr>>>, calls: Arc<AtomicUsize>) -> Scope {
    let host = scope::tests::host(&[("192.168.1.20", Some(24))], Some("192.168.1.20"));
    Scope {
        network: host.current_network().unwrap(),
        resolve: Box::new(move |name, port| {
            assert_eq!((name, port), ("printer.local", 80));
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(answers.lock().unwrap().clone())
        }),
        host: Box::new(move || Ok(host.clone())),
        source: Box::new(|_| Some(Ipv4Addr::new(192, 168, 1, 20))),
        rules: std::sync::RwLock::default(),
    }
}

fn lan_prepared(answers: Arc<Mutex<Vec<SocketAddr>>>, calls: Arc<AtomicUsize>) -> Result<Prepared, Error> {
    let endpoint = Endpoint::new("Printer.Local.", 80).unwrap();
    let source = Source::lan_fixture(lan_scope(answers, calls));
    let binding = source.observe(&endpoint)?;
    Ok(Prepared {
        attempt: Attempt::new(),
        endpoint,
        source,
        binding,
    })
}

fn ambiguous_lan_answers() -> [Vec<SocketAddr>; 3] {
    let first = "192.168.1.50:80".parse().unwrap();
    [
        vec![first, "203.0.113.50:80".parse().unwrap()],
        vec![first, "[::ffff:203.0.113.50]:80".parse().unwrap()],
        vec![first, first, first, first, "192.168.1.60:80".parse().unwrap()],
    ]
}

#[test]
fn a_single_terminal_dot_is_normalized_during_actual_endpoint_admission() {
    let address = "192.168.1.50:80".parse().unwrap();
    let answers = Arc::new(Mutex::new(vec![address]));
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Gate {
        prepared: lan_prepared(answers, Arc::clone(&calls)).unwrap(),
        reason: Mutex::new(None),
    };
    for name in ["Printer.Local.", "printer.local", "PRINTER.LOCAL."] {
        assert_eq!(gate.admit(&Destination::Name(name.into(), 80)), Ok(vec![address]));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    for (name, port) in [("printer.local..", 80), ("other.local.", 80), ("printer.local.", 81)] {
        assert!(gate.admit(&Destination::Name(name.into(), port)).is_err());
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        4,
        "refused names must cause no source lookup"
    );
}

#[test]
fn complete_lan_answers_must_be_unique_before_policy_filters_or_caps_them() {
    for answers in ambiguous_lan_answers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let result = lan_prepared(Arc::new(Mutex::new(answers)), Arc::clone(&calls));
        assert!(matches!(result, Err(Error::InvalidEndpoint)));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn duplicate_only_lan_answers_bind_once_without_a_second_resolution() {
    let address = "192.168.1.50:80".parse().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let candidate = lan_prepared(Arc::new(Mutex::new(vec![address; 8])), Arc::clone(&calls)).unwrap();
    assert_eq!(candidate.binding.address(), address);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn ambiguity_between_preflight_and_start_never_opens_a_gateway() {
    for ambiguous in ambiguous_lan_answers() {
        let answers = Arc::new(Mutex::new(vec!["192.168.1.50:80".parse().unwrap()]));
        let candidate = lan_prepared(Arc::clone(&answers), Arc::new(AtomicUsize::new(0))).unwrap();
        *answers.lock().unwrap() = ambiguous;
        assert!(matches!(Gateway::start(candidate), Err(Error::InvalidEndpoint)));
    }
}

#[test]
fn later_lan_ambiguity_revokes_admission_and_cannot_reopen_after_dns_recovers() {
    for ambiguous in ambiguous_lan_answers() {
        let address = "192.168.1.50:80".parse().unwrap();
        let answers = Arc::new(Mutex::new(vec![address]));
        let calls = Arc::new(AtomicUsize::new(0));
        let candidate = lan_prepared(Arc::clone(&answers), calls).unwrap();
        let gateway = Gateway::start(candidate).unwrap();
        let request = Destination::Name("Printer.Local.".into(), 80);
        assert_eq!(gateway.gate.admit(&request), Ok(vec![address]));
        *answers.lock().unwrap() = ambiguous;
        assert!(gateway.gate.admit(&request).is_err());
        assert_eq!(
            gateway.status().state,
            State::Revoked {
                reason: Reason::SourceUnavailable
            }
        );
        assert!(!gateway.gate.keeps(address));
        *answers.lock().unwrap() = vec![address];
        assert!(gateway.gate.admit(&request).is_err(), "a new approval is required");
    }
}

#[test]
fn lan_policy_is_rechecked_after_the_single_dns_lookup() {
    let address = "192.168.1.50:80".parse().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut scope = lan_scope(Arc::new(Mutex::new(vec![address])), Arc::clone(&calls));
    let moved = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&moved);
    let original_host = scope.host;
    scope.host = Box::new(move || {
        let mut host = original_host()?;
        if observed.load(Ordering::Acquire) {
            host.route = None;
        }
        Ok(host)
    });
    let original_resolve = scope.resolve;
    scope.resolve = Box::new(move |name, port| {
        let answers = original_resolve(name, port);
        moved.store(true, Ordering::Release);
        answers
    });
    let source = Source::lan_fixture(scope);
    assert_eq!(
        source.observe(&Endpoint::new("printer.local", 80).unwrap()),
        Err(Error::SourceUnavailable)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
