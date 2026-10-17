use super::*;
use crate::trusted_tcp_producer::{pending_fixture, pending_transport_fixture};
use tokio::net::UnixStream;

async fn unix_end(stream: &mut UnixStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = timeout(WAIT, stream.read_to_end(&mut bytes)).await.unwrap();
    bytes
}

fn named_request(port: u16) -> ExposureRequest {
    let mut input = request(port);
    input.mode = ExposureMode::Restricted;
    input.sources = vec![SourceRule::Tag("tag:test".into())];
    input
}

#[test]
fn producer_ports_are_paired_and_cannot_alias_the_direct_ingress() {
    for virtual_port in RESERVED_INGRESS_START..=RESERVED_INGRESS_END {
        let backend = horizon_cloud_protocol::exposure::producer_backend_port(virtual_port).unwrap();
        let public = if virtual_port == RESERVED_INGRESS_START {
            virtual_port + 1
        } else {
            RESERVED_INGRESS_START
        };
        assert!(trusted::ports(public, virtual_port, backend).is_ok());
        assert!(trusted::ports(virtual_port, virtual_port, backend).is_err());
        assert!(trusted::ports(public, virtual_port, backend + 1).is_err());
    }
    for (public, virtual_port, backend) in [(0, 20000, 20032), (20032, 20000, 20032), (20000, 0, 20032)] {
        assert!(trusted::ports(public, virtual_port, backend).is_err());
    }
}

#[tokio::test]
async fn a_registered_accept_cookie_cannot_be_replaced_by_a_foreign_pending_stream() {
    let (first, mut first_client) = UnixStream::pair().unwrap();
    let first = pending_fixture(first);
    let original = first.cancellation();
    let retained = original.clone();
    let (other, _other_client) = UnixStream::pair().unwrap();
    let other = pending_fixture(other);
    let (control, mut daemon_control) = UnixStream::pair().unwrap();
    let accepted = pending_transport_fixture(first, control);
    assert!(trusted::same_accept(&original, &accepted));
    assert!(!trusted::same_accept(&other.cancellation(), &accepted));
    retained.cancel();
    assert!(unix_end(&mut first_client).await.is_empty());
    assert!(unix_end(&mut daemon_control).await.is_empty());
}

#[tokio::test]
async fn a_retained_pending_handle_does_not_keep_the_original_payload_open() {
    let (payload, mut client) = UnixStream::pair().unwrap();
    let pending = pending_fixture(payload);
    let retained = pending.cancellation();
    drop(pending);
    assert!(unix_end(&mut client).await.is_empty());
    retained.cancel();
}

#[tokio::test]
async fn cancelling_pending_registration_releases_queued_rights_with_its_owner_retained() {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
    use std::{io::IoSlice, mem::MaybeUninit, os::fd::AsFd};

    let (payload, sender) = UnixStream::pair().unwrap();
    let pending = pending_fixture(payload);
    let retained = pending.cancellation();
    let (endpoint, mut control_peer) = UnixStream::pair().unwrap();
    {
        let descriptors = [endpoint.as_fd()];
        let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut storage);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
        assert_eq!(
            rustix::net::sendmsg(&sender, &[IoSlice::new(b"P")], &mut ancillary, SendFlags::NOSIGNAL).unwrap(),
            1
        );
    }
    drop(endpoint);
    let mut byte = [0];
    assert_eq!(
        control_peer.try_read(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let shared = Arc::new(Mutex::new(State::new(
        Vec::new(),
        named_request(18080).validate().unwrap(),
        Budgets::default(),
        Instant::now() + WAIT,
    )));
    let id = lock(&shared).reserve_pending(&pending).unwrap();
    lock(&shared).cancel_flow(id);
    assert_eq!(timeout(WAIT, control_peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    retained.cancel();
    drop(pending);
    lock(&shared).release(id);
}

#[tokio::test]
async fn gate_pending_cancellation_physically_closes_a_retained_transitioned_transport() {
    let (payload, mut client) = UnixStream::pair().unwrap();
    let pending = pending_fixture(payload);
    let retained = pending.cancellation();
    let shared = Arc::new(Mutex::new(State::new(
        Vec::new(),
        named_request(18080).validate().unwrap(),
        Budgets::default(),
        Instant::now() + WAIT,
    )));
    let id = lock(&shared).reserve_pending(&pending).unwrap();
    let (control, mut daemon_control) = UnixStream::pair().unwrap();
    let accepted = Arc::new(pending_transport_fixture(pending, control));
    assert!(trusted::same_accept(&retained, &accepted));
    lock(&shared).cancel_flow(id);
    assert!(unix_end(&mut client).await.is_empty());
    assert!(unix_end(&mut daemon_control).await.is_empty());
    assert!(timeout(WAIT, accepted.finish()).await.unwrap().is_err());
    lock(&shared).complete(id);
    lock(&shared).release(id);
    assert_eq!(lock(&shared).counters().active_connections, 0);
}

#[tokio::test]
async fn mixed_rules_on_direct_sockets_never_trust_a_forwarded_name_tag_or_ip() {
    for allow_cidr in [false, true] {
        let server = backend().await;
        let mut input = named_request(server.local_addr().unwrap().port());
        input.sources.extend([
            SourceRule::DnsName("peer.example.ts.net".into()),
            SourceRule::Cidr(if allow_cidr { "127.0.0.1/32" } else { "192.0.2.0/24" }.into()),
        ]);
        let (gate, address) = fixture(&input, Budgets::default());
        let mut client = connect(address).await;
        if allow_cidr {
            let mut service = accepted(&server).await;
            client.write_all(b"CIDR").await.unwrap();
            let mut bytes = [0; 4];
            timeout(WAIT, service.read_exact(&mut bytes)).await.unwrap().unwrap();
            assert_eq!(&bytes, b"CIDR");
        } else {
            let _ = client
                .write_all(b"PROXY TCP4 192.0.2.1 100.64.0.2 12345 20032\r\npeer.example.ts.net tag:test")
                .await;
            assert!(end(&mut client).await.is_empty());
            assert!(timeout(Duration::from_millis(100), server.accept()).await.is_err());
        }
        assert_eq!(lock(&gate.shared).verified_peers, 0);
        gate.close().await.unwrap();
    }
}

#[tokio::test]
async fn pending_identity_has_no_payload_or_backend_capacity_and_aborts_before_poll() {
    let (original, mut client) = UnixStream::pair().unwrap();
    let original = pending_fixture(original);
    let mut input = named_request(18080);
    input.limits = ConnectionLimits {
        total: 1,
        per_source: 1,
    };
    let shared = Arc::new(Mutex::new(State::new(
        Vec::new(),
        input.validate().unwrap(),
        Budgets::default(),
        Instant::now() + WAIT,
    )));
    let id = lock(&shared).reserve_pending(&original).unwrap();
    assert_eq!(lock(&shared).reserve(id, 4), 0);
    let (denied, _other) = UnixStream::pair().unwrap();
    let denied = pending_fixture(denied);
    assert!(lock(&shared).reserve_pending(&denied).is_none());
    let listener = backend().await;
    let _backend_peer = connect(listener.local_addr().unwrap()).await;
    let denied_backend = Arc::new(accepted(&listener).await);
    assert!(!lock(&shared).attach(id, denied_backend));
    let flow = relay::Flow {
        shared: Arc::clone(&shared),
        id,
    };
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        std::future::pending::<()>().await;
        drop(flow);
    });
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    assert_eq!(lock(&shared).counters().active_connections, 0);
    assert_eq!(lock(&shared).counters().charged_bytes, 0);
    assert!(
        unix_end(&mut client).await.is_empty(),
        "registration shuts even a retained original pending owner"
    );
    drop(original);
}

#[tokio::test]
async fn identity_wait_obeys_cancel_phase_deadline_and_failure_without_minting_a_peer() {
    for case in 0..4 {
        let (original, mut client) = UnixStream::pair().unwrap();
        let original = pending_fixture(original);
        let shared = Arc::new(Mutex::new(State::new(
            Vec::new(),
            named_request(18080).validate().unwrap(),
            Budgets::default(),
            Instant::now() + WAIT,
        )));
        let id = lock(&shared).reserve_pending(&original).unwrap();
        let flow = relay::Flow {
            shared: Arc::clone(&shared),
            id,
        };
        let (stop, cancel) = watch::channel(false);
        match case {
            0 => {
                stop.send(true).unwrap();
            }
            1 => lock(&shared).cancel_flow(id),
            2 => lock(&shared).until = Instant::now() + Duration::from_millis(30),
            _ => {}
        }
        let observed = if case == 3 {
            trusted::identify_pending(
                &flow,
                cancel,
                std::future::ready(Err(crate::trusted_tcp_producer::Error::Encoding)),
            )
            .await
        } else {
            timeout(WAIT, trusted::identify_pending(&flow, cancel, std::future::pending()))
                .await
                .unwrap()
        };
        assert!(observed.is_none());
        drop(flow);
        assert_eq!(lock(&shared).verified_peers, 0);
        assert_eq!(lock(&shared).counters().active_connections, 0);
        assert!(unix_end(&mut client).await.is_empty());
        drop(original);
    }
}

#[tokio::test]
#[ignore = "Requires approved original Root daemon/mapping plus separate actual tailnet peer; allow and deny in both modes"]
async fn actual_task_owned_named_admission_uses_the_original_opaque_peer() {
    use crate::trusted_tcp_producer::{Producer, Runtime};
    let expected_peer_id = std::env::var("HORIZON_TRUSTED_GATE_EXPECTED_PEER_ID").unwrap();
    let runtime = match std::env::var("HORIZON_TRUSTED_GATE_COMPONENT").unwrap().as_str() {
        "task-owned-worker" => Runtime::Worker,
        "task-owned-kernel" => Runtime::Kernel,
        _ => panic!("Explicit approved component required"),
    };
    let allow = match std::env::var("HORIZON_TRUSTED_GATE_EXPECT").unwrap().as_str() {
        "allow" => true,
        "deny" => false,
        _ => panic!("Explicit allow or deny observation required"),
    };
    let request = ExposureRequest::from_json(std::env::var("HORIZON_TRUSTED_GATE_POLICY").unwrap().as_bytes()).unwrap();
    assert_eq!(request.mode, ExposureMode::Restricted);
    assert_eq!(request.target_port, 18080);
    assert!(request.validate().unwrap().has_named_sources());
    let service = TcpListener::bind((Ipv4Addr::LOCALHOST, 18080)).await.unwrap();
    let producer = Producer::pin(runtime, 20000, 20032).await.unwrap();
    let backend_path = producer.backend_path().to_owned();
    let gate = Gate::bind_with_producer(
        20001,
        producer,
        &request,
        Budgets {
            gate_lifetime: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(10),
            ..Budgets::default()
        },
    )
    .unwrap();
    assert_eq!(gate.producer_backend(), Some(backend_path.as_path()));
    assert!(
        timeout(WAIT, TcpStream::connect((Ipv4Addr::LOCALHOST, 20032)))
            .await
            .unwrap()
            .is_err()
    );
    let mut forged = UnixStream::connect(&backend_path).await.unwrap();
    // Wrong kernel credentials may close this original stream before any header write.
    let _ = forged
        .write_all(b"PROXY TCP4 100.64.0.1 100.64.0.2 12345 20000\r\nPING")
        .await;
    assert!(unix_end(&mut forged).await.is_empty());
    inactive(&gate).await;
    assert_eq!(lock(&gate.shared).verified_peers, 0);
    assert!(timeout(Duration::from_millis(100), service.accept()).await.is_err());
    let refused = gate.counters().refused;
    let policy_denials = lock(&gate.shared).policy_denials;
    let normal_completed = lock(&gate.shared).normal_completed;
    println!("horizon-trusted-gate-ready=public:20001,virtual:20000,backend:unix");
    if allow {
        let mut original = timeout(Duration::from_secs(20), service.accept())
            .await
            .unwrap()
            .unwrap()
            .0;
        let mut bytes = [0; 4];
        timeout(WAIT, original.read_exact(&mut bytes)).await.unwrap().unwrap();
        assert_eq!(&bytes, b"PING");
        original.write_all(b"PONG").await.unwrap();
        timeout(WAIT, original.read_exact(&mut bytes)).await.unwrap().unwrap();
        assert_eq!(&bytes, b"DONE");
        assert!(end(&mut original).await.is_empty());
        original.shutdown().await.unwrap();
        inactive(&gate).await;
        assert_eq!(
            lock(&gate.shared).normal_completed.checked_sub(normal_completed),
            Some(1)
        );
        let counters = gate.counters();
        assert_eq!(counters.charged_bytes, 12);
        assert_eq!(counters.forwarded_bytes, 12);
        println!("horizon-trusted-gate-normal-drained=1");
    } else {
        timeout(Duration::from_secs(20), async {
            while lock(&gate.shared).verified_peers == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        inactive(&gate).await;
        assert!(gate.counters().refused > refused);
        assert_eq!(
            lock(&gate.shared).policy_denials.checked_sub(policy_denials),
            Some(1),
            "capacity, timeout or custody refusal is not named-policy denial"
        );
        assert!(timeout(Duration::from_millis(100), service.accept()).await.is_err());
    }
    {
        let observed = lock(&gate.shared);
        assert_eq!(
            observed.verified_peers, 1,
            "exactly the separate original peer is required"
        );
        assert_eq!(observed.verified_peer_id.as_deref(), Some(expected_peer_id.as_str()));
    }
    gate.close().await.unwrap();
    println!("horizon-trusted-gate-original-closed=1");
}
