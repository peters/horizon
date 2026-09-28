use super::*;
use crate::cloud_runtime::local_network::{Device, Rules};

/// A discoverer on [`probe_scope`] narrowed to the printer on `ports`, recording each dial.
fn narrowed(ports: &[u16]) -> (Discoverer, Arc<Mutex<Vec<SocketAddr>>>) {
    let scope = probe_scope();
    *scope.rules.write().unwrap() = Rules {
        devices: vec![Device {
            address: v4("192.168.1.50"),
            ports: ports.to_vec(),
        }],
        local_ports: Vec::new(),
    };
    let dials = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&dials);
    let discoverer = Discoverer::with_parts(
        scope,
        Box::new(|_, _, _: &Cancellation| (Vec::new(), Vec::new())),
        Box::new(move |target: SocketAddr, _, _: &Cancellation| {
            seen.lock().unwrap().push(target);
            Ok(())
        }),
        Cancellation::default(),
    );
    (discoverer, dials)
}

fn probe(discoverer: &Discoverer, host: &str, ports: &[u16]) -> Answer {
    discoverer.answer(Request::Probe {
        host: host.into(),
        ports: ports.to_vec(),
    })
}

#[test]
fn a_probe_of_a_narrowed_device_dials_only_the_ports_the_scope_reaches() {
    let (discoverer, dials) = narrowed(&[80, 631, 8443]);
    let Answer::Probe(result) = probe(&discoverer, "printer.local", &[]) else {
        panic!("probe of the defaults");
    };
    let expected: Vec<u16> = DEFAULT_PROBE_PORTS
        .iter()
        .copied()
        .filter(|port| [80, 631, 8443].contains(port))
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(result.open, expected, "only the allowed defaults are probed");
    assert!(
        dials
            .lock()
            .unwrap()
            .iter()
            .all(|target| expected.contains(&target.port()))
    );
    let Answer::Refused(refusal) = probe(&discoverer, "printer.local", &[80, 9100]) else {
        panic!("a named port outside the scope");
    };
    assert!(refusal.contains("Port 9100 on printer.local"), "{refusal}");
    assert_eq!(
        dials.lock().unwrap().len(),
        expected.len(),
        "a refused probe dials nothing"
    );
    assert!(matches!(probe(&discoverer, "192.168.1.60", &[80]), Answer::Refused(_)));
}

#[test]
fn a_device_open_on_no_default_port_refuses_a_probe_of_the_defaults() {
    let (discoverer, dials) = narrowed(&[40_000]);
    assert!(matches!(probe(&discoverer, "printer.local", &[]), Answer::Refused(_)));
    assert!(dials.lock().unwrap().is_empty());
    assert!(matches!(
        probe(&discoverer, "printer.local", &[40_000]),
        Answer::Probe(_)
    ));
}

#[test]
fn narrowing_the_scope_hides_devices_already_found_or_probed() {
    let scope = probe_scope();
    let discoverer = Discoverer::with_parts(
        Arc::clone(&scope),
        Box::new(|_, _, _: &Cancellation| (vec![Finding::Seen(v4("192.168.1.50"), Source::Mdns)], Vec::new())),
        Box::new(|_, _, _: &Cancellation| Ok(())),
        Cancellation::default(),
    );
    // Found by a browse, and a second device known only from a probe.
    assert!(matches!(probe(&discoverer, "192.168.1.60", &[22]), Answer::Probe(_)));
    let Answer::Discovery(before) = discoverer.discover() else {
        panic!("discovery");
    };
    assert_eq!(before.devices.len(), 2);
    scope
        .set_rules(
            Rules {
                devices: vec![Device {
                    address: v4("192.168.1.50"),
                    ports: Vec::new(),
                }],
                local_ports: Vec::new(),
            },
            41234,
        )
        .unwrap();
    // The cached answer is reused, but only with what the rules now reach.
    let Answer::Discovery(after) = discoverer.discover() else {
        panic!("discovery");
    };
    assert_eq!(
        after.devices.iter().map(|device| device.address).collect::<Vec<_>>(),
        [v4("192.168.1.50")]
    );
}

#[test]
fn a_probe_stops_before_dialling_ports_the_owner_closed_while_it_ran() {
    let scope = probe_scope();
    let dials = Arc::new(Mutex::new(Vec::new()));
    let discoverer = {
        let (scope, seen) = (Arc::clone(&scope), Arc::clone(&dials));
        Discoverer::with_parts(
            Arc::clone(&scope),
            Box::new(|_, _, _: &Cancellation| (Vec::new(), Vec::new())),
            Box::new(move |target: SocketAddr, _, _: &Cancellation| {
                seen.lock().unwrap().push(target.port());
                // The owner narrows the printer to its web port while the first batch runs.
                *scope.rules.write().unwrap() = Rules {
                    devices: vec![Device {
                        address: v4("192.168.1.50"),
                        ports: vec![80],
                    }],
                    local_ports: Vec::new(),
                };
                Ok(())
            }),
            Cancellation::default(),
        )
    };
    let ports: Vec<u16> = (8000..8008).collect();
    let Answer::Refused(refusal) = probe(&discoverer, "printer.local", &ports) else {
        panic!("the probe went on after the scope changed");
    };
    assert_eq!(refusal, probe::NARROWED);
    let dialled = dials.lock().unwrap().clone();
    assert!(dialled.iter().all(|port| (8000..8004).contains(port)), "{dialled:?}");
}

#[test]
fn devices_out_of_scope_never_crowd_an_allowed_one_out_of_the_answer() {
    // A /16 network, which has room for more devices than one answer lists.
    let home = || scope::tests::host(&[("10.0.0.20", Some(16))], Some("10.0.0.20"));
    let scope = Arc::new(Scope {
        network: home().current_network().unwrap(),
        resolve: Box::new(|_, _| Err(Reply::HostUnreachable)),
        host: Box::new(move || Ok(home())),
        source: Box::new(|_| Some(v4("10.0.0.20"))),
        rules: std::sync::RwLock::default(),
    });
    // More named devices than an answer holds; named devices are listed first.
    let crowd = |_: Ipv4Addr, _: Subnet, _: &Cancellation| {
        let findings = (0..300u16)
            .map(|index| {
                let [high, low] = (index + 256).to_be_bytes();
                Finding::Name(Ipv4Addr::new(10, 0, high, low), format!("device-{index}.local"))
            })
            .collect();
        (findings, Vec::new())
    };
    let discoverer = Discoverer::with_parts(
        Arc::clone(&scope),
        Box::new(crowd),
        Box::new(|_, _, _: &Cancellation| Ok(())),
        Cancellation::default(),
    );
    // Known only from a probe, so it is listed after every named device.
    assert!(matches!(probe(&discoverer, "10.0.200.5", &[22]), Answer::Probe(_)));
    let Answer::Discovery(crowded) = discoverer.discover() else {
        panic!("discovery");
    };
    assert!(crowded.truncated);
    assert!(!crowded.devices.iter().any(|device| device.address == v4("10.0.200.5")));
    scope
        .set_rules(
            Rules {
                devices: vec![Device {
                    address: v4("10.0.200.5"),
                    ports: Vec::new(),
                }],
                local_ports: Vec::new(),
            },
            41234,
        )
        .unwrap();
    let Answer::Discovery(narrowed) = discoverer.discover() else {
        panic!("discovery");
    };
    assert_eq!(
        narrowed.devices.iter().map(|device| device.address).collect::<Vec<_>>(),
        [v4("10.0.200.5")]
    );
}

#[test]
fn a_name_with_several_addresses_probes_one_the_scope_allows_on_those_ports() {
    let twin = Arc::new(Scope {
        resolve: Box::new(|_, port| {
            Ok(vec![
                SocketAddr::new(v4("192.168.1.50").into(), port),
                SocketAddr::new(v4("192.168.1.60").into(), port),
            ])
        }),
        ..Arc::into_inner(probe_scope()).unwrap()
    });
    *twin.rules.write().unwrap() = Rules {
        devices: vec![
            Device {
                address: v4("192.168.1.50"),
                ports: vec![80],
            },
            Device {
                address: v4("192.168.1.60"),
                ports: vec![22],
            },
        ],
        local_ports: Vec::new(),
    };
    let discoverer = Discoverer::with_parts(
        twin,
        Box::new(|_, _, _: &Cancellation| (Vec::new(), Vec::new())),
        Box::new(|_, _, _: &Cancellation| Ok(())),
        Cancellation::default(),
    );
    let Answer::Probe(ssh) = probe(&discoverer, "twin.local", &[22]) else {
        panic!("the second address allows port 22");
    };
    assert_eq!((ssh.address, ssh.open), (v4("192.168.1.60"), vec![22]));
    let Answer::Probe(defaults) = probe(&discoverer, "twin.local", &[]) else {
        panic!("defaults");
    };
    assert_eq!(
        defaults.address,
        v4("192.168.1.50"),
        "the first address allowing any default"
    );
    assert!(matches!(
        probe(&discoverer, "twin.local", &[22, 80]),
        Answer::Refused(_)
    ));
}
