use super::*;

fn camera_only(ports: &[u16], local_ports: &[u16]) -> Rules {
    Rules {
        devices: vec![Device {
            address: Ipv4Addr::new(192, 168, 1, 50),
            ports: ports.to_vec(),
        }],
        local_ports: local_ports.to_vec(),
    }
}

/// A scope on the home network whose names `camera.local` and `other.local` resolve to two
/// devices, and `sneaky.local` to this computer's loopback.
fn named() -> Scope {
    gate(Box::new(|name, port| {
        Ok(match name {
            "camera.local" => addresses(&["192.168.1.50:0"]),
            "other.local" => addresses(&["192.168.1.60:0"]),
            "sneaky.local" => addresses(&["127.0.0.1:0"]),
            _ => return Err(Reply::HostUnreachable),
        }
        .into_iter()
        .map(|address| SocketAddr::new(address.ip(), port))
        .collect())
    }))
}

#[test]
fn a_narrowed_scope_reaches_only_its_devices_and_ports() {
    let scope = named();
    scope.set_rules(camera_only(&[554], &[]), 41234).unwrap();
    assert_eq!(
        scope.admit(&Destination::Address("192.168.1.50:554".parse().unwrap())),
        Ok(addresses(&["192.168.1.50:554"]))
    );
    assert_eq!(
        scope.admit(&name("camera.local", 554)),
        Ok(addresses(&["192.168.1.50:554"]))
    );
    for refused in [
        Destination::Address("192.168.1.50:80".parse().unwrap()),
        Destination::Address("192.168.1.60:554".parse().unwrap()),
        name("other.local", 554),
    ] {
        assert_eq!(scope.admit(&refused), Err(Reply::NotAllowed), "{refused:?}");
    }
    // A probe asks about the device whatever the port, and discovery lists only the device.
    assert_eq!(
        scope.admit_host(&Destination::Address("192.168.1.50:1".parse().unwrap())),
        Ok(addresses(&["192.168.1.50:1"]))
    );
    assert_eq!(
        scope.admit_host(&Destination::Address("192.168.1.60:1".parse().unwrap())),
        Err(Reply::NotAllowed)
    );
    let found = [Ipv4Addr::new(192, 168, 1, 50), Ipv4Addr::new(192, 168, 1, 60)].into();
    assert_eq!(scope.reachable(&found), Ok([Ipv4Addr::new(192, 168, 1, 50)].into()));
    // Widening again restores the whole network.
    scope.set_rules(Rules::default(), 41234).unwrap();
    assert_eq!(
        scope.admit(&name("other.local", 80)),
        Ok(addresses(&["192.168.1.60:80"]))
    );
}

#[test]
fn this_computer_is_reachable_only_on_the_loopback_ports_the_owner_opened() {
    let scope = named();
    let local = |port| Destination::Name("localhost".into(), port);
    assert_eq!(scope.admit(&local(3000)), Err(Reply::NotAllowed), "closed by default");
    scope.set_rules(camera_only(&[], &[3000]), 41234).unwrap();
    assert_eq!(
        scope.admit(&local(3000)),
        Ok(addresses(&["127.0.0.1:3000", "[::1]:3000"]))
    );
    assert_eq!(
        scope.admit(&Destination::Address("127.0.0.1:3000".parse().unwrap())),
        Ok(addresses(&["127.0.0.1:3000"]))
    );
    assert_eq!(
        scope.admit(&Destination::Address("[::1]:3000".parse().unwrap())),
        Ok(addresses(&["[::1]:3000"]))
    );
    for refused in [
        local(22),
        Destination::Address("127.0.0.1:22".parse().unwrap()),
        Destination::Address("127.0.0.2:3000".parse().unwrap()),
        // Only the exact loopback literals open a port, not their IPv4-mapped form.
        Destination::Address("[::ffff:127.0.0.1]:3000".parse().unwrap()),
        // This computer's address on the network, and a name that resolves to its loopback,
        // stay refused: only the loopback itself is opened, port by port.
        Destination::Address("192.168.1.20:3000".parse().unwrap()),
        name("sneaky.local", 3000),
    ] {
        assert_eq!(scope.admit(&refused), Err(Reply::NotAllowed), "{refused:?}");
    }
    assert_eq!(
        scope.admit_host(&local(3000)),
        Err(Reply::HostUnreachable),
        "a probe never reaches this computer"
    );
    assert!(scope.keeps("127.0.0.1:3000".parse().unwrap()));
    assert!(!scope.keeps("127.0.0.1:22".parse().unwrap()));
}

#[test]
fn loopback_ports_close_with_the_network() {
    let scope = Scope {
        host: Box::new(|| Ok(scope::tests::host(&[("10.1.0.9", Some(24))], Some("10.1.0.9")))),
        ..named()
    };
    *scope.rules.write().unwrap() = camera_only(&[], &[3000]);
    assert_eq!(
        scope.admit(&Destination::Name("localhost".into(), 3000)),
        Err(Reply::NetworkUnreachable)
    );
}

#[test]
fn rules_that_name_something_the_bridge_cannot_reach_are_refused_and_the_old_ones_stay() {
    let scope = named();
    scope.set_rules(camera_only(&[554], &[]), 41234).unwrap();
    for (address, why) in [
        (Ipv4Addr::new(10, 0, 0, 5), "another network"),
        (Ipv4Addr::new(192, 168, 1, 20), "this computer"),
        (Ipv4Addr::new(192, 168, 1, 255), "the broadcast address"),
    ] {
        let rules = Rules {
            devices: vec![Device {
                address,
                ports: Vec::new(),
            }],
            local_ports: Vec::new(),
        };
        assert_eq!(
            scope.set_rules(rules, 41234),
            Err(RulesError::OutsideNetwork(address)),
            "{why}"
        );
    }
    assert_eq!(
        scope.set_rules(camera_only(&[], &[41234]), 41234),
        Err(RulesError::BridgePort(41234))
    );
    assert_eq!(scope.rules(), camera_only(&[554], &[]));
}
