use super::*;
use crate::cloud_runtime::local_network::scope;
use horizon_cloud_protocol::local_network::discovery::DEFAULT_PROBE_PORTS;
use simple_dns::{
    CLASS, Label, Name, Packet, PacketFlag, ResourceRecord,
    rdata::{A, PTR, RData, SRV, TXT},
};
use std::{
    net::{IpAddr, UdpSocket},
    sync::atomic::{AtomicUsize, Ordering},
};

fn v4(value: &str) -> Ipv4Addr {
    value.parse().unwrap()
}

fn record(name: Name<'static>, rdata: RData<'static>) -> ResourceRecord<'static> {
    ResourceRecord::new(name, CLASS::IN, 120, rdata)
}

fn name(value: &'static str) -> Name<'static> {
    Name::new_unchecked(value)
}

/// An instance name whose first label holds characters DNS host names cannot.
fn instance(label: &'static str, kind: &'static str) -> Name<'static> {
    let mut labels = vec![Label::new_unchecked(label.as_bytes())];
    labels.extend(kind.split('.').map(|part| Label::new_unchecked(part.as_bytes())));
    Name::new_with_labels(&labels)
}

fn response(answers: Vec<ResourceRecord<'static>>, additional: Vec<ResourceRecord<'static>>) -> Vec<u8> {
    let mut packet = Packet::new_reply(0);
    packet.set_flags(PacketFlag::AUTHORITATIVE_ANSWER);
    packet.answers = answers;
    packet.additional_records = additional;
    packet.build_bytes_vec_compressed().unwrap()
}

fn questions(packets: &[Vec<u8>]) -> Vec<String> {
    packets
        .iter()
        .flat_map(|bytes| {
            let packet = Packet::parse(bytes).unwrap();
            assert!(!packet.has_flags(PacketFlag::RESPONSE));
            packet
                .questions
                .iter()
                .map(|question| format!("{} {:?}", question.qname, question.qtype))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_browse_asks_for_types_then_instances_then_hosts_and_never_twice() {
    let mut browse = mdns::Browse::default();
    let first = questions(&browse.first_queries(&[v4("192.168.1.60")]));
    assert!(first.contains(&"_services._dns-sd._udp.local TYPE(PTR)".to_owned()));
    assert!(first.contains(&"_ipp._tcp.local TYPE(PTR)".to_owned()));
    assert!(first.contains(&"60.1.168.192.in-addr.arpa TYPE(PTR)".to_owned()));

    browse.absorb(&response(
        vec![
            record(
                name("_services._dns-sd._udp.local"),
                RData::PTR(PTR(name("_arduino._tcp.local"))),
            ),
            record(
                name("_services._dns-sd._udp.local"),
                RData::PTR(PTR(name("_ipp._tcp.local"))),
            ),
            // Not a service type, so not asked about.
            record(
                name("_services._dns-sd._udp.local"),
                RData::PTR(PTR(name("printer.local"))),
            ),
            record(
                name("_arduino._tcp.local"),
                RData::PTR(PTR(instance("Desk Board 3D", "_arduino._tcp.local"))),
            ),
        ],
        Vec::new(),
    ));
    let second = questions(&browse.next_queries());
    assert!(
        second.contains(&"_arduino._tcp.local TYPE(PTR)".to_owned()),
        "{second:?}"
    );
    assert!(
        !second.iter().any(|question| question.starts_with("_ipp._tcp.local")),
        "asked in the first round"
    );
    assert!(second.contains(&"Desk Board 3D._arduino._tcp.local TYPE(SRV)".to_owned()));
    assert!(second.contains(&"Desk Board 3D._arduino._tcp.local TYPE(TXT)".to_owned()));

    browse.absorb(&response(
        vec![record(
            instance("Desk Board 3D", "_arduino._tcp.local"),
            RData::SRV(SRV {
                priority: 0,
                weight: 0,
                port: 5000,
                target: name("uno.local"),
            }),
        )],
        Vec::new(),
    ));
    let third = questions(&browse.next_queries());
    assert_eq!(third, ["uno.local TYPE(A)"]);
    assert!(browse.next_queries().is_empty());
}

#[test]
fn a_response_places_services_names_and_reverse_names_at_their_addresses() {
    let mut browse = mdns::Browse::default();
    let printer = instance("Office\u{1b} Printer.2nd floor", "_ipp._tcp.local");
    let mut txt = TXT::new();
    txt.add_string("ty=Brother HL-L2350DW").unwrap();
    txt.add_string("rp=ipp/print").unwrap();
    txt.add_string("=no-key").unwrap();
    txt.add_char_string(simple_dns::CharacterString::new(b"pk=\xff\xfe").unwrap());
    browse.absorb(&response(
        vec![record(name("_ipp._tcp.local"), RData::PTR(PTR(printer.clone())))],
        vec![
            record(
                printer,
                RData::SRV(SRV {
                    priority: 0,
                    weight: 0,
                    port: 631,
                    target: name("brother.local"),
                }),
            ),
            record(
                instance("Office\u{1b} Printer.2nd floor", "_ipp._tcp.local"),
                RData::TXT(txt),
            ),
            record(name("brother.local"), RData::A(A::from(v4("192.168.1.50")))),
            record(
                name("60.1.168.192.in-addr.arpa"),
                RData::PTR(PTR(name("raspberrypi.local"))),
            ),
        ],
    ));
    // A query, and bytes that are no DNS message at all, teach nothing.
    browse.absorb(&Packet::new_query(1).build_bytes_vec().unwrap());
    browse.absorb(b"\x00\x01garbage");
    let findings = browse.findings();
    let printer = v4("192.168.1.50");
    assert!(findings.contains(&Finding::Name(printer, "brother.local".into())));
    assert!(findings.contains(&Finding::Name(v4("192.168.1.60"), "raspberrypi.local".into())));
    let service = findings
        .iter()
        .find_map(|finding| match finding {
            Finding::Service(address, service) if *address == printer => Some(service.clone()),
            _ => None,
        })
        .expect("printer service");
    assert_eq!(service.kind, "_ipp._tcp");
    assert_eq!(service.name.as_deref(), Some("Office Printer.2nd floor"));
    assert_eq!(service.port, Some(631));
    assert_eq!(
        service.attributes.get("ty").map(String::as_str),
        Some("Brother HL-L2350DW")
    );
    assert_eq!(service.attributes.len(), 2);
}

#[test]
fn ssdp_answers_describe_each_device_once_by_its_most_telling_type() {
    let from = v4("192.168.1.70");
    let answer = |kind: &str, location: &str| {
        format!(
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: {location}\r\nSERVER: Linux/5.4 UPnP/1.0 Sonos/80.1\r\nST: {kind}\r\nUSN: uuid:RINCON::{kind}\r\n\r\n"
        )
    };
    let description = "http://192.168.1.70:1400/xml/device_description.xml";
    let answers: Vec<_> = [
        answer("upnp:rootdevice", description),
        answer("urn:schemas-upnp-org:device:ZonePlayer:1", description),
        answer("urn:schemas-upnp-org:service:AVTransport:1", description),
    ]
    .iter()
    .map(|text| (from, ssdp::parse(from, text.as_bytes()).unwrap()))
    .collect();
    let findings = ssdp::findings(answers);
    assert_eq!(findings.len(), 2);
    let Finding::Service(address, service) = &findings[1] else {
        panic!("{findings:?}");
    };
    assert_eq!(*address, from);
    assert_eq!(service.kind, "urn:schemas-upnp-org:device:ZonePlayer:1");
    assert_eq!(service.port, Some(1400));
    assert_eq!(service.attributes["location"], description);
    assert!(service.attributes["server"].contains("Sonos"));

    // A description on another host is not reachable through this device, so it is dropped.
    let elsewhere = ssdp::parse(from, answer("upnp:rootdevice", "http://10.0.0.9/desc.xml").as_bytes()).unwrap();
    let findings = ssdp::findings([(from, elsewhere)]);
    let Finding::Service(_, service) = &findings[1] else {
        panic!("{findings:?}");
    };
    assert!(!service.attributes.contains_key("location"));
    assert_eq!(service.port, None);
    assert!(ssdp::parse(from, b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\n\r\n").is_none());
    assert!(ssdp::parse(from, b"HTTP/1.1 500 Error\r\nST: upnp:rootdevice\r\n\r\n").is_none());
    assert!(ssdp::parse(from, b"HTTP/1.1 200 OK\r\nSERVER: x\r\n\r\n").is_none());
}

#[test]
fn neighbor_tables_list_resolved_addresses_on_every_system() {
    let linux = "IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         a4:91:b1:00:11:22     *        wlp2s0
192.168.1.77     0x1         0x0         00:00:00:00:00:00     *        wlp2s0
192.168.1.50     0x1         0x6         3c:2a:f4:01:02:03     *        wlp2s0
";
    assert_eq!(neighbors::linux(linux), [v4("192.168.1.1"), v4("192.168.1.50")]);
    let bsd = "? (192.168.1.1) at a4:91:b1:0:11:22 on en0 ifscope [ethernet]
? (192.168.1.77) at (incomplete) on en0 ifscope [ethernet]
raspberrypi.lan (192.168.1.60) at dc:a6:32:1:2:3 on en0 ifscope [ethernet]
? (192.168.1.255) at ff:ff:ff:ff:ff:ff on en0 ifscope [ethernet]
";
    assert_eq!(neighbors::bsd(bsd), [v4("192.168.1.1"), v4("192.168.1.60")]);
    let windows = "
Schnittstelle: 192.168.1.20 --- 0x7
  Internetadresse       Physische Adresse     Typ
  192.168.1.1           a4-91-b1-00-11-22     dynamisch
  192.168.1.255         ff-ff-ff-ff-ff-ff     statisch
  224.0.0.251           01-00-5e-00-00-fb     statisch
";
    assert_eq!(neighbors::windows(windows), [v4("192.168.1.1"), v4("224.0.0.251")]);
}

fn service(kind: &str, port: Option<u16>) -> Service {
    Service {
        source: Source::Mdns,
        kind: kind.into(),
        name: None,
        port,
        attributes: BTreeMap::new(),
    }
}

#[test]
fn findings_merge_per_address_and_only_admitted_addresses_remain() {
    let printer = v4("192.168.1.50");
    let findings = vec![
        Finding::Seen(printer, Source::Neighbors),
        Finding::Name(printer, "brother.local".into()),
        Finding::Name(printer, "brother.local".into()),
        Finding::Service(printer, service("_ipp._tcp", Some(631))),
        Finding::Service(printer, service("_ipp._tcp", Some(631))),
        Finding::Service(printer, service("_http._tcp", Some(80))),
        Finding::Service(printer, service("_sleep-proxy._udp", Some(55124))),
        Finding::Seen(v4("10.0.0.5"), Source::Ssdp),
    ];
    let devices = merge(findings, |addresses| {
        assert_eq!(addresses.len(), 2);
        Ok(addresses
            .iter()
            .copied()
            .filter(|address| *address == printer)
            .collect())
    })
    .unwrap();
    assert_eq!(devices.len(), 1);
    let device = &devices[0];
    assert_eq!(device.names, ["brother.local"]);
    assert_eq!(device.services.len(), 3);
    assert_eq!(device.ports, [631, 80]);
    assert_eq!(device.sources, [Source::Mdns, Source::Neighbors]);
    assert_eq!(
        merge(vec![Finding::Seen(printer, Source::Mdns)], |_| Err(
            Reply::NetworkUnreachable
        )),
        Err(Reply::NetworkUnreachable)
    );
}

/// A scope on the home network whose host answers from `host`, with every destination routed
/// out of the bridged interface.
fn scope(host: impl Fn() -> io::Result<scope::Host> + Send + Sync + 'static) -> Arc<Scope> {
    let home = scope::tests::host(&[("192.168.1.20", Some(24))], Some("192.168.1.20"));
    Arc::new(Scope {
        network: home.current_network().unwrap(),
        resolve: Box::new(|_, _| Err(Reply::HostUnreachable)),
        host: Box::new(host),
        source: Box::new(|_| Some(v4("192.168.1.20"))),
    })
}

#[test]
fn browses_are_shared_scoped_and_never_run_off_the_bridged_network() {
    let home = || Ok(scope::tests::host(&[("192.168.1.20", Some(24))], Some("192.168.1.20")));
    let browses = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&browses);
    let discoverer = Discoverer::with_browse(
        scope(home),
        Box::new(move |local, subnet| {
            counted.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                (local, subnet.to_string().as_str()),
                (v4("192.168.1.20"), "192.168.1.0/24")
            );
            let findings = [
                "192.168.1.50",
                "192.168.1.20",
                "192.168.1.255",
                "10.0.0.5",
                "224.0.0.251",
            ]
            .into_iter()
            .map(|address| Finding::Seen(v4(address), Source::Neighbors))
            .collect();
            (findings, vec!["SSDP search failed".into()])
        }),
    );
    let Answer::Discovery(first) = discoverer.discover() else {
        panic!("discovery");
    };
    let addresses: Vec<_> = first.devices.iter().map(|device| device.address).collect();
    assert_eq!(addresses, [v4("192.168.1.50")]);
    assert_eq!(first.notes, ["SSDP search failed"]);
    let Answer::Discovery(second) = discoverer.discover() else {
        panic!("discovery");
    };
    assert_eq!(second.devices, first.devices);
    assert_eq!(
        browses.load(Ordering::SeqCst),
        1,
        "a second request within the reuse window shares the browse"
    );

    let moved = Discoverer::with_browse(
        scope(|| Ok(scope::tests::host(&[("10.0.0.20", Some(24))], Some("10.0.0.20")))),
        Box::new(|_, _| panic!("browsed a network that is not the bridged one")),
    );
    assert_eq!(
        moved.discover(),
        Answer::Refused(Reply::NetworkUnreachable.message().into())
    );
}

/// Sends real mDNS and SSDP queries on this computer's network, so it only runs on request:
/// `cargo test -p horizon-core a_browse_of_this -- --ignored`.
#[test]
#[ignore = "browses the network this computer is on"]
fn a_browse_of_this_computers_network_ends_within_its_window() {
    // Computers without a shareable network have nothing to browse.
    let Ok(current) = Scope::current() else {
        return;
    };
    let started = Instant::now();
    let answer = Discoverer::new(Arc::new(current)).discover();
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
    match answer {
        Answer::Discovery(discovery) => assert!(discovery.devices.len() <= 256),
        // The runner's network can change while the test runs.
        Answer::Refused(_) => {}
        Answer::Probe(_) => panic!("a discovery answered with a probe"),
    }
}

/// Stands in for the devices on the network: answers every datagram it receives with what
/// `reply` returns, and counts the datagrams. It stops after a few quiet seconds.
fn responder(reply: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> (SocketAddr, Arc<AtomicUsize>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let address = socket.local_addr().unwrap();
    let received = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&received);
    thread::spawn(move || {
        let mut buffer = [0; 9000];
        while let Ok((count, from)) = socket.recv_from(&mut buffer) {
            counted.fetch_add(1, Ordering::SeqCst);
            for datagram in reply(&buffer[..count]) {
                let _ = socket.send_to(&datagram, from);
            }
        }
    });
    (address, received)
}

#[test]
fn an_mdns_browse_keeps_its_rounds_and_packet_limit_under_a_flood() {
    let (group, queries) = responder(|bytes| {
        let Ok(query) = Packet::parse(bytes) else {
            return Vec::new();
        };
        let mut datagrams = Vec::new();
        if query
            .questions
            .iter()
            .any(|question| question.qname.to_string() == "_ipp._tcp.local")
        {
            let printer = instance("Office", "_ipp._tcp.local");
            datagrams.push(response(
                vec![record(name("_ipp._tcp.local"), RData::PTR(PTR(printer.clone())))],
                vec![
                    record(
                        printer,
                        RData::SRV(SRV {
                            priority: 0,
                            weight: 0,
                            port: 631,
                            target: name("brother.local"),
                        }),
                    ),
                    record(name("brother.local"), RData::A(A::from(v4("192.168.1.50")))),
                ],
            ));
        }
        // More than a whole browse may read, after every query.
        datagrams.extend((0..mdns::MAX_PACKETS).map(|_| b"junk".to_vec()));
        datagrams
    });
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let started = Instant::now();
    let findings = mdns::exchange(&socket, group, &[]).unwrap();
    let window: Duration = mdns::ROUNDS.iter().sum();
    assert!(
        started.elapsed() < window + Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(findings.contains(&Finding::Name(v4("192.168.1.50"), "brother.local".into())));
    assert!(queries.load(Ordering::SeqCst) >= 2, "the first round spans two packets");
}

#[test]
fn an_ssdp_search_repeats_once_and_ends_with_its_window() {
    let (group, searches) = responder(|bytes| {
        if !bytes.starts_with(b"M-SEARCH") {
            return Vec::new();
        }
        vec![b"HTTP/1.1 200 OK\r\nLOCATION: http://127.0.0.1:1400/description.xml\r\nST: urn:schemas-upnp-org:device:ZonePlayer:1\r\n\r\n".to_vec()]
    });
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let started = Instant::now();
    let findings = ssdp::search(&socket, group).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= ssdp::WINDOW && elapsed < ssdp::WINDOW + Duration::from_millis(500),
        "{elapsed:?}"
    );
    assert_eq!(searches.load(Ordering::SeqCst), 2);
    assert!(findings.iter().any(|finding| matches!(
        finding,
        Finding::Service(address, service) if *address == v4("127.0.0.1") && service.port == Some(1400)
    )));
}

/// `arp` on macOS and Windows runs through the same bounded runner.
#[cfg(unix)]
#[test]
fn the_neighbor_command_reports_failures_and_bounds_its_output_and_time() {
    let shell = |script: &str| {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    };
    assert_eq!(neighbors::run(shell("printf 'table\\n'")).unwrap(), "table\n");
    let failed = neighbors::run(shell("printf 'arp: not permitted\\n' >&2; exit 2"))
        .unwrap_err()
        .to_string();
    assert!(failed.contains("ended with"), "{failed}");
    // A long table is cut, and the command still finishes.
    let long = neighbors::run(shell("head -c 1000000 /dev/zero | tr '\\0' 'a'")).unwrap();
    assert_eq!(long.len(), 256 * 1024);
    let started = Instant::now();
    let slow = neighbors::run(shell("exec sleep 10")).unwrap_err().to_string();
    assert!(slow.contains("too long"), "{slow}");
    assert!(started.elapsed() < Duration::from_secs(4));
}

/// The home network, where `printer.local` resolves to a device and `rebound.local` to this
/// computer's loopback.
fn probe_scope() -> Arc<Scope> {
    let home = || scope::tests::host(&[("192.168.1.20", Some(24))], Some("192.168.1.20"));
    Arc::new(Scope {
        network: home().current_network().unwrap(),
        resolve: Box::new(|name, port| match name {
            "printer.local" => Ok(vec![SocketAddr::new(v4("192.168.1.50").into(), port)]),
            "rebound.local" => Ok(vec![SocketAddr::new(v4("127.0.0.1").into(), port)]),
            // A valid name longer than the text limit for device-chosen strings.
            long if long.len() == 200 => Ok(vec![SocketAddr::new(v4("192.168.1.60").into(), port)]),
            _ => Err(Reply::HostUnreachable),
        }),
        host: Box::new(move || Ok(home())),
        source: Box::new(|_| Some(v4("192.168.1.20"))),
    })
}

#[test]
fn probes_connect_only_to_admitted_hosts_a_few_times_a_minute() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&attempts);
    let discoverer = Discoverer::with_parts(
        probe_scope(),
        Box::new(|_, _| (Vec::new(), Vec::new())),
        Box::new(move |target: SocketAddr, _| {
            seen.lock().unwrap().push(target);
            match target.port() {
                80 | 631 => Ok(()),
                22 => Err(io::ErrorKind::ConnectionRefused.into()),
                _ => Err(io::ErrorKind::TimedOut.into()),
            }
        }),
    );
    let probe = |host: &str, ports: &[u16]| {
        discoverer.answer(Request::Probe {
            host: host.into(),
            ports: ports.to_vec(),
        })
    };
    // Malformed requests and hosts outside the scope are refused before anything is sent,
    // and do not count towards the rate.
    let many: Vec<u16> = (1..=17).collect();
    for (host, ports) in [
        ("", &[][..]),
        ("printer.local", &[0][..]),
        ("printer.local", &many[..]),
        ("10.0.0.5", &[][..]),
        ("192.168.1.20", &[][..]),
        ("192.168.1.255", &[][..]),
        ("127.0.0.1", &[][..]),
        ("rebound.local", &[][..]),
        ("missing.local", &[][..]),
    ] {
        assert!(matches!(probe(host, ports), Answer::Refused(_)), "{host} {ports:?}");
    }
    assert!(attempts.lock().unwrap().is_empty());

    let Answer::Probe(result) = probe("printer.local", &[22, 80, 631, 9100, 80]) else {
        panic!("probe");
    };
    assert_eq!(result.address, v4("192.168.1.50"));
    assert_eq!(
        (result.open, result.closed, result.silent),
        (vec![80, 631], vec![22], vec![9100])
    );
    assert!(matches!(probe("192.168.1.50", &[]), Answer::Probe(_)));
    let attempted = attempts.lock().unwrap().clone();
    assert_eq!(attempted.len(), 4 + DEFAULT_PROBE_PORTS.len());
    assert!(
        attempted
            .iter()
            .all(|target| target.ip() == IpAddr::from(v4("192.168.1.50")))
    );

    // Later discovery answers list the ports probes found open.
    let Answer::Discovery(found) = discoverer.discover() else {
        panic!("discovery");
    };
    assert_eq!(found.devices.len(), 1);
    assert_eq!(found.devices[0].ports, [80, 631]);
    assert_eq!(found.devices[0].sources, [Source::Probe]);

    for _ in 2..probe::PER_MINUTE {
        assert!(matches!(probe("192.168.1.50", &[80]), Answer::Probe(_)));
    }
    let Answer::Refused(limit) = probe("192.168.1.50", &[80]) else {
        panic!("the rate limit held no probe back");
    };
    assert!(limit.contains("6 probes a minute"), "{limit}");
}

#[test]
fn probed_ports_join_advertised_ones_and_hosts_come_back_as_named() {
    let printer = v4("192.168.1.50");
    let discoverer = Discoverer::with_parts(
        probe_scope(),
        Box::new(move |_, _| {
            let findings = vec![
                Finding::Seen(printer, Source::Mdns),
                Finding::Service(printer, service("_ipp._tcp", Some(631))),
            ];
            (findings, Vec::new())
        }),
        Box::new(|target: SocketAddr, _| match target.port() {
            80 | 631 => Ok(()),
            _ => Err(io::ErrorKind::ConnectionRefused.into()),
        }),
    );
    let Answer::Probe(_) = discoverer.answer(Request::Probe {
        host: "printer.local".into(),
        ports: vec![631, 80],
    }) else {
        panic!("probe");
    };
    let Answer::Discovery(found) = discoverer.discover() else {
        panic!("discovery");
    };
    assert!(
        !found.truncated,
        "an advertised port found open again is not a truncation"
    );
    assert_eq!(found.devices[0].ports, [80, 631]);
    assert_eq!(found.devices[0].sources, [Source::Mdns, Source::Probe]);

    // Names may be up to 253 characters, longer than device text, and come back whole.
    let long = format!(
        "{label}.{label}.{label}.{}.local",
        "a".repeat(47),
        label = "a".repeat(48)
    );
    assert_eq!(long.len(), 200);
    let Answer::Probe(result) = discoverer.answer(Request::Probe {
        host: long.clone(),
        ports: vec![80],
    }) else {
        panic!("probe of a long name");
    };
    assert_eq!((result.host, result.address), (long, v4("192.168.1.60")));
}
