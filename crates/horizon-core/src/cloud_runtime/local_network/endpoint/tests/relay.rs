use super::*;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    time::Duration,
};

pub(super) fn connect(gateway: &Gateway, address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, gateway.status().receipt.proxy_port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.write_all(&[5, 1, 0]).unwrap();
    let mut choice = [0; 2];
    stream.read_exact(&mut choice).unwrap();
    assert_eq!(choice, [5, 0]);
    let IpAddr::V4(ip) = address.ip() else {
        panic!("fixture IPv4");
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend(ip.octets());
    request.extend(address.port().to_be_bytes());
    stream.write_all(&request).unwrap();
    let mut reply = [0; 10];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply[1], 0);
    stream
}

#[test]
fn actual_relay_preserves_tcp_bytes_and_half_close() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap();
        assert_eq!(received, b"request\x00body");
        stream.write_all(b"response-after-eof").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
    });
    let candidate = prepared(address);
    let attempt = candidate.attempt();
    let gateway = Gateway::start(candidate).unwrap();
    assert_eq!(gateway.status().receipt.operation_id, attempt.operation_id);
    assert_eq!(gateway.status().receipt.generation, attempt.generation);
    let mut stream = connect(&gateway, address);
    stream.write_all(b"request\x00body").unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut received = Vec::new();
    stream.read_to_end(&mut received).unwrap();
    assert_eq!(received, b"response-after-eof");
    upstream.join().unwrap();
    assert!(gateway.status().bytes >= 12 + 18);
    let json = serde_json::to_value(gateway.status()).unwrap();
    assert_eq!(json["source"], "tailnet");
    assert!(json.get("peer").is_none() && json.get("desktop").is_none());
}

#[test]
fn source_identity_change_closes_an_existing_relay_and_refuses_new_connections() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&changed);
    let mut candidate = prepared(address);
    candidate.source = Source::fixture(move |_| {
        Ok(binding(
            address,
            if observed.load(Ordering::Acquire) {
                "replacement"
            } else {
                "peer-fixture"
            },
        ))
    });
    let gateway = Gateway::start(candidate).unwrap();
    let mut downstream = connect(&gateway, address);
    let (mut upstream, _) = listener.accept().unwrap();
    upstream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    downstream.write_all(b"before").unwrap();
    let mut bytes = [0; 6];
    upstream.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"before");
    changed.store(true, Ordering::Release);
    let end = Instant::now() + Duration::from_secs(3);
    while gateway.status().state == State::Active && Instant::now() < end {
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        gateway.status().state,
        State::Revoked {
            reason: Reason::BindingChanged
        }
    );
    let mut one = [0];
    assert_eq!(upstream.read(&mut one).unwrap(), 0);
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, gateway.status().receipt.proxy_port)).is_err()
            || gateway.gate.admit(&Destination::Address(address)).is_err()
    );
}

#[test]
fn stale_receipt_at_a_reused_port_cannot_close_a_replacement_gateway() {
    let old = Gateway::start(prepared("127.0.0.1:18080".parse().unwrap())).unwrap();
    let mut stale = old.status().receipt;
    drop(old);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let replacement = Gateway::start(prepared(address)).unwrap();
    // Simulate port reuse while retaining the old operation/generation, as a delayed receipt does.
    stale.proxy_port = replacement.status().receipt.proxy_port;
    assert_eq!(replacement.revoke(&stale), Err(Error::StaleReceipt));
    let mut downstream = connect(&replacement, address);
    let (mut upstream, _) = listener.accept().unwrap();
    downstream.write_all(b"alive").unwrap();
    upstream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut bytes = [0; 5];
    upstream.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"alive");
    let exact = replacement.status().receipt;
    replacement.revoke(&exact).unwrap();
    replacement.revoke(&exact).unwrap();
    assert_eq!(
        replacement.status().state,
        State::Revoked {
            reason: Reason::OwnerClosed
        }
    );
    let mut one = [0];
    assert_eq!(upstream.read(&mut one).unwrap(), 0);
}
