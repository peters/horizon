use super::*;
use std::net::TcpListener;

/// Admits configured loopback fixtures, which the real scope never allows, and names
/// that map to them; records what it was asked.
#[derive(Default)]
struct FixtureGate {
    allowed: Vec<SocketAddr>,
    names: HashMap<String, Vec<SocketAddr>>,
    asked: Mutex<Vec<Destination>>,
    /// Admitted addresses whose relays the current rules no longer keep.
    revoked: Mutex<Vec<SocketAddr>>,
}

impl Gate for FixtureGate {
    fn admit(&self, destination: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(destination.clone());
        match destination {
            Destination::Address(address) if self.allowed.contains(address) => Ok(vec![*address]),
            Destination::Name(name, port) => self
                .names
                .get(name)
                .map(|addresses| {
                    addresses
                        .iter()
                        .map(|address| SocketAddr::new(address.ip(), *port))
                        .collect()
                })
                .ok_or(Reply::HostUnreachable),
            Destination::Address(_) => Err(Reply::NotAllowed),
        }
    }

    fn keeps(&self, address: SocketAddr) -> bool {
        !self
            .revoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&address)
    }
}

fn echo_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut buffer = [0; 1024];
            while let Ok(count) = socket.read(&mut buffer) {
                if count == 0 || socket.write_all(&buffer[..count]).is_err() {
                    break;
                }
            }
        }
    });
    (address, server)
}

fn proxy(gate: FixtureGate) -> (Proxy, Arc<FixtureGate>) {
    let gate = Arc::new(gate);
    (Proxy::start(Arc::clone(&gate) as Arc<dyn Gate>).unwrap(), gate)
}

fn client(proxy: &Proxy) -> TcpStream {
    let socket = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    socket
}

fn greet(socket: &mut TcpStream) {
    socket.write_all(&[5, 2, 2, 0]).unwrap();
    let mut choice = [0; 2];
    socket.read_exact(&mut choice).unwrap();
    assert_eq!(choice, [5, 0]);
}

fn ipv4_request(address: SocketAddr) -> Vec<u8> {
    let IpAddr::V4(ip) = address.ip() else {
        panic!("IPv4 fixture")
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend(ip.octets());
    request.extend(address.port().to_be_bytes());
    request
}

fn name_request(name: &[u8], port: u16) -> Vec<u8> {
    let mut request = vec![5, 1, 0, 3, u8::try_from(name.len()).unwrap()];
    request.extend(name);
    request.extend(port.to_be_bytes());
    request
}

fn reply(socket: &mut TcpStream) -> u8 {
    let mut reply = [0; 10];
    socket.read_exact(&mut reply).unwrap();
    assert_eq!((reply[0], reply[2], reply[3]), (5, 0, 1));
    reply[1]
}

/// The proxy closed the connection; a read timeout does not count.
fn closed(socket: &mut TcpStream) -> bool {
    let mut byte = [0; 1];
    match socket.read(&mut byte) {
        Ok(count) => count == 0,
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
        ),
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_admitted_address_relays_both_directions_and_counts_bytes() {
    let (echo, server) = echo_server();
    let (proxy, _) = proxy(FixtureGate {
        allowed: vec![echo],
        ..FixtureGate::default()
    });
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&ipv4_request(echo)).unwrap();
    assert_eq!(reply(&mut socket), 0);
    socket.write_all(b"camera frame").unwrap();
    let mut echoed = [0; 12];
    socket.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"camera frame");
    assert_eq!(proxy.counters().connections, 1);
    // The echo can arrive before its bytes are added to the relay's count.
    wait_for(|| proxy.relays().first().is_some_and(|relay| relay.bytes == 24));
    let relays = proxy.relays();
    assert_eq!(relays.len(), 1);
    assert_eq!(
        (&relays[0].requested, relays[0].address),
        (&Destination::Address(echo), echo)
    );
    socket.shutdown(Shutdown::Write).unwrap();
    assert!(closed(&mut socket));
    server.join().unwrap();
    wait_for(|| proxy.counters().connections == 0);
    assert!(proxy.relays().is_empty(), "an ended relay is no longer listed");
    assert_eq!(
        proxy.counters(),
        Counters {
            connections: 0,
            bytes: 24,
            refused: 0
        }
    );
}

#[test]
fn names_go_to_the_gate_and_only_its_addresses_are_dialled() {
    let (echo, server) = echo_server();
    let (proxy, gate) = proxy(FixtureGate {
        names: HashMap::from([("camera.local".to_owned(), vec![echo])]),
        ..FixtureGate::default()
    });
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&name_request(b"camera.local", echo.port())).unwrap();
    assert_eq!(reply(&mut socket), 0);
    socket.write_all(b"x").unwrap();
    let mut echoed = [0; 1];
    socket.read_exact(&mut echoed).unwrap();
    let relays = proxy.relays();
    assert_eq!(
        (&relays[0].requested, relays[0].address),
        (&Destination::Name("camera.local".into(), echo.port()), echo),
        "a name is listed as asked, with the address it was dialled at"
    );
    drop(socket);
    server.join().unwrap();
    assert_eq!(
        *gate.asked.lock().unwrap(),
        vec![Destination::Name("camera.local".into(), echo.port())]
    );
}

#[test]
fn ipv6_and_literal_addresses_in_name_form_are_checked_as_addresses() {
    let (proxy, gate) = proxy(FixtureGate::default());
    let mut ipv6 = vec![5, 1, 0, 4];
    ipv6.extend(Ipv6Addr::LOCALHOST.octets());
    ipv6.extend(80_u16.to_be_bytes());
    for (request, expected) in [
        (name_request(b"192.168.1.50", 80), "192.168.1.50:80"),
        (name_request(b"::ffff:127.0.0.1", 80), "[::ffff:127.0.0.1]:80"),
        (ipv6, "[::1]:80"),
    ] {
        let mut socket = client(&proxy);
        greet(&mut socket);
        socket.write_all(&request).unwrap();
        assert_eq!(reply(&mut socket), Reply::NotAllowed.code());
        assert!(closed(&mut socket));
        let asked = gate.asked.lock().unwrap().pop().unwrap();
        assert_eq!(asked, Destination::Address(expected.parse().unwrap()));
    }
}

#[test]
fn a_refused_destination_is_never_dialled_and_a_closed_port_is_refused_by_the_device() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    // Held until the proxy has its own port, so the proxy cannot be given this one.
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = reserved.local_addr().unwrap();
    let (proxy, _) = proxy(FixtureGate {
        allowed: vec![closed_port],
        ..FixtureGate::default()
    });
    drop(reserved);
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&ipv4_request(closed_port)).unwrap();
    assert_eq!(reply(&mut socket), Reply::ConnectionRefused.code());
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&ipv4_request(listener.local_addr().unwrap())).unwrap();
    assert_eq!(reply(&mut socket), Reply::NotAllowed.code());
    assert!(closed(&mut socket));
    assert!(
        listener
            .accept()
            .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
    );
    assert_eq!(proxy.counters().refused, 2);
}

#[test]
fn unsupported_requests_get_their_own_reply_codes() {
    let (proxy, gate) = proxy(FixtureGate::default());
    let cases: [(Vec<u8>, Reply); 7] = [
        (vec![5, 2, 0, 1, 127, 0, 0, 1, 0, 80], Reply::CommandNotSupported),
        (vec![5, 3, 0, 1, 127, 0, 0, 1, 0, 80], Reply::CommandNotSupported),
        (vec![5, 1, 0, 9], Reply::AddressTypeNotSupported),
        (vec![5, 1, 1, 1], Reply::GeneralFailure),
        (vec![4, 1, 0, 1], Reply::GeneralFailure),
        (name_request(b"bad name", 80), Reply::AddressTypeNotSupported),
        (vec![5, 1, 0, 1, 192, 168, 1, 50, 0, 0], Reply::NotAllowed),
    ];
    for (request, expected) in cases {
        let mut socket = client(&proxy);
        greet(&mut socket);
        socket.write_all(&request).unwrap();
        assert_eq!(reply(&mut socket), expected.code(), "{request:?}");
        assert!(closed(&mut socket));
    }
    assert!(gate.asked.lock().unwrap().is_empty());
    wait_for(|| proxy.counters().refused == 7);
}

#[test]
fn only_socks5_without_authentication_is_spoken() {
    let (proxy, _) = proxy(FixtureGate::default());
    // A client that already sent its request still reads the refusal.
    for greeting in [&[5, 1, 2][..], &[5, 0], &[5, 1, 2, 5, 1, 0, 1, 127, 0, 0, 1, 0, 80]] {
        let mut socket = client(&proxy);
        socket.write_all(greeting).unwrap();
        let mut choice = [0; 2];
        socket.read_exact(&mut choice).unwrap();
        assert_eq!(choice, [5, 0xff]);
        assert!(closed(&mut socket));
    }
    for greeting in [&[4, 1, 0, 80, 127, 0, 0, 1, 0][..], b"GET / HTTP/1.1\r\n\r\n"] {
        let mut socket = client(&proxy);
        socket.write_all(greeting).unwrap();
        assert!(closed(&mut socket), "{greeting:?}");
    }
}

#[test]
fn names_follow_dns_syntax() {
    for valid in [
        "camera",
        "camera.local",
        "camera.local.",
        "my_printer.lan",
        "a-b.example",
    ] {
        assert!(
            matches!(name_destination(valid.as_bytes()), Some(Destination::Name(..))),
            "{valid}"
        );
    }
    let (long_label, long_name) = ("a".repeat(64), format!("{}b", "a.".repeat(127)));
    // 253 octets, with or without the root dot.
    assert!(name_destination(format!("{}b", "a.".repeat(126)).as_bytes()).is_some());
    assert!(name_destination(format!("{}b.", "a.".repeat(126)).as_bytes()).is_some());
    for invalid in [
        "",
        ".",
        "a..b",
        ".a",
        "-a.b",
        "a-.b",
        "camera-.local",
        "a b",
        "a/b",
        "a:b",
        "a\0b",
    ] {
        assert_eq!(name_destination(invalid.as_bytes()), None, "{invalid:?}");
    }
    assert_eq!(name_destination(long_label.as_bytes()), None);
    assert_eq!(name_destination(long_name.as_bytes()), None);
    assert_eq!(name_destination(&[0xff, 0xfe]), None);
}

#[test]
fn a_trickled_handshake_is_closed_at_its_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let trickle = thread::spawn(move || {
        for byte in [5, 1, 0, 5, 1] {
            if writer.write_all(&[byte]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(80));
        }
    });
    let started = Instant::now();
    let result = negotiate(&mut server, started + Duration::from_millis(200));
    assert!(matches!(result, Err(Refusal::Silent)));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(server);
    trickle.join().unwrap();
}

#[test]
fn connections_beyond_the_limit_are_closed_and_counted() {
    let (proxy, _) = proxy(FixtureGate::default());
    let stalled: Vec<_> = (0..MAX_CONNECTIONS).map(|_| client(&proxy)).collect();
    wait_for(|| proxy.counters().connections == MAX_CONNECTIONS);
    let mut extra = client(&proxy);
    assert!(closed(&mut extra));
    assert!(proxy.counters().refused >= 1);
    drop(proxy);
    for mut socket in stalled {
        assert!(closed(&mut socket));
    }
}

#[test]
fn stopping_the_proxy_ends_open_relays() {
    let (echo, server) = echo_server();
    let (proxy, _) = proxy(FixtureGate {
        allowed: vec![echo],
        ..FixtureGate::default()
    });
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&ipv4_request(echo)).unwrap();
    assert_eq!(reply(&mut socket), 0);
    let shared = Arc::clone(&proxy.shared);
    wait_for(|| !shared.relays.is_empty());
    let started = Instant::now();
    drop(proxy);
    assert!(closed(&mut socket));
    assert!(started.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
    drop(socket);
    wait_for(|| shared.active.load(Ordering::Acquire) == 0);
    assert!(shared.relays.is_empty());
}

#[test]
fn the_byte_budget_refuses_new_connections_and_ends_every_open_one() {
    let (echo, server) = echo_server();
    let (other, other_server) = echo_server();
    let (proxy, _) = proxy(FixtureGate {
        allowed: vec![echo, other],
        ..FixtureGate::default()
    });
    let mut open = client(&proxy);
    greet(&mut open);
    open.write_all(&ipv4_request(echo)).unwrap();
    assert_eq!(reply(&mut open), 0);
    let mut idle = client(&proxy);
    greet(&mut idle);
    idle.write_all(&ipv4_request(other)).unwrap();
    assert_eq!(reply(&mut idle), 0);
    proxy.shared.bytes.store(BYTE_BUDGET - 4, Ordering::Release);
    // Exactly the rest of the budget is relayed, then every relay ends.
    open.write_all(b"more").unwrap();
    assert!(closed(&mut open));
    assert!(closed(&mut idle));
    assert_eq!(proxy.counters().bytes, BYTE_BUDGET);
    let mut late = client(&proxy);
    greet(&mut late);
    late.write_all(&ipv4_request(echo)).unwrap();
    assert_eq!(reply(&mut late), Reply::GeneralFailure.code());
    server.join().unwrap();
    other_server.join().unwrap();
}

#[test]
fn narrowed_rules_close_the_relays_they_no_longer_allow_and_leave_the_rest() {
    let (kept, kept_server) = echo_server();
    let (dropped, dropped_server) = echo_server();
    let (proxy, _) = proxy(FixtureGate {
        allowed: vec![kept, dropped],
        ..FixtureGate::default()
    });
    let open = |target| {
        let mut socket = client(&proxy);
        greet(&mut socket);
        socket.write_all(&ipv4_request(target)).unwrap();
        assert_eq!(reply(&mut socket), 0);
        socket
    };
    let (mut stays, mut goes) = (open(kept), open(dropped));
    wait_for(|| proxy.relays().len() == 2);
    proxy.close_unless(|address| address == kept);
    assert!(closed(&mut goes));
    // The worker's end closes once it sees the close, as `stopping_the_proxy_ends_open_relays`
    // does; only then do the relay's threads end on every platform.
    drop(goes);
    dropped_server.join().unwrap();
    stays.write_all(b"still here").unwrap();
    let mut echoed = [0; 10];
    stays.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"still here");
    wait_for(|| proxy.relays().iter().map(|relay| relay.address).eq([kept]));
    drop(stays);
    kept_server.join().unwrap();
}

#[test]
fn a_relay_the_rules_stopped_allowing_before_it_was_listed_is_closed_at_once() {
    let (echo, server) = echo_server();
    let (proxy, gate) = proxy(FixtureGate {
        allowed: vec![echo],
        ..FixtureGate::default()
    });
    // Admitted by the gate, then no longer kept: as if the owner narrowed the scope between
    // the admission and the relay being listed, where a sweep cannot see it.
    gate.revoked.lock().unwrap().push(echo);
    let mut socket = client(&proxy);
    greet(&mut socket);
    socket.write_all(&ipv4_request(echo)).unwrap();
    assert_eq!(reply(&mut socket), 0);
    assert!(closed(&mut socket));
    wait_for(|| proxy.counters().connections == 0);
    assert!(proxy.relays().is_empty());
    drop(socket);
    server.join().unwrap();
}
