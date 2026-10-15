use super::super::socks::Gate as _;
use super::*;
use std::sync::atomic::AtomicUsize;

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
