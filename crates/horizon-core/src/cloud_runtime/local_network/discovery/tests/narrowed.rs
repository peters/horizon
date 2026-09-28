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
