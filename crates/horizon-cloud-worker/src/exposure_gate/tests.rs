use super::*;
use horizon_cloud_protocol::exposure::{ConnectionLimits, SourceRule};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpSocket,
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(4);
fn request(port: u16) -> ExposureRequest {
    ExposureRequest {
        mode: ExposureMode::Public,
        target_port: port,
        sources: Vec::new(),
        limits: ConnectionLimits::default(),
    }
}
pub(super) async fn backend() -> TcpListener {
    for _ in 0..32 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        if request(listener.local_addr().unwrap().port()).validate().is_ok() {
            return listener;
        }
    }
    panic!("No non-management fixture port was available");
}
fn fixture(request: &ExposureRequest, budgets: Budgets) -> (Gate, SocketAddr) {
    let policy = request.validate().unwrap();
    let listener = bind((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let address = listener.local_addr().unwrap();
    (
        Gate::owned(vec![listener], policy, budgets.validate().unwrap()).unwrap(),
        address,
    )
}
async fn connect(address: SocketAddr) -> TcpStream {
    timeout(WAIT, TcpStream::connect(address)).await.unwrap().unwrap()
}
async fn connect_from(address: SocketAddr, source: [u8; 4]) -> TcpStream {
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind((Ipv4Addr::from(source), 0).into()).unwrap();
    timeout(WAIT, socket.connect(address)).await.unwrap().unwrap()
}
async fn accepted(listener: &TcpListener) -> TcpStream {
    timeout(WAIT, listener.accept()).await.unwrap().unwrap().0
}
async fn end(stream: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = timeout(WAIT, stream.read_to_end(&mut bytes)).await.unwrap();
    bytes
}
async fn inactive(gate: &Gate) {
    timeout(WAIT, async {
        while gate.counters().active_connections != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn raw_bytes_and_request_half_close_preserve_the_response() {
    let server = backend().await;
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), Budgets::default());
    let mut client = connect(address).await;
    let mut service = accepted(&server).await;
    client.write_all(b"request").await.unwrap();
    client.shutdown().await.unwrap();
    let mut request_bytes = Vec::new();
    timeout(WAIT, service.read_to_end(&mut request_bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request_bytes, b"request");
    service.write_all(b"response").await.unwrap();
    service.shutdown().await.unwrap();
    let mut response = Vec::new();
    timeout(WAIT, client.read_to_end(&mut response)).await.unwrap().unwrap();
    assert_eq!(response, b"response");
    inactive(&gate).await;
    assert_eq!(gate.counters().forwarded_bytes, 15);
    gate.close().await.unwrap();
}

#[tokio::test]
async fn actual_socket_peer_controls_cidr_and_forged_proxy_headers_do_not() {
    let server = backend().await;
    let mut input = request(server.local_addr().unwrap().port());
    input.mode = ExposureMode::Restricted;
    input.sources = vec![SourceRule::Cidr("192.0.2.0/24".into())];
    let (gate, address) = fixture(&input, Budgets::default());
    let mut client = connect(address).await;
    let _ = client.write_all(b"PROXY TCP4 192.0.2.1 127.0.0.1 1234 8080\r\n").await;
    assert!(end(&mut client).await.is_empty());
    assert!(timeout(Duration::from_millis(100), server.accept()).await.is_err());
    assert_eq!(gate.counters().refused, 1);
    gate.close().await.unwrap();
    input.sources = vec![SourceRule::Cidr("127.0.0.1/32".into())];
    let (gate, address) = fixture(&input, Budgets::default());
    let mut client = connect(address).await;
    let mut service = accepted(&server).await;
    client.write_all(b"ok").await.unwrap();
    let mut bytes = [0; 2];
    timeout(WAIT, service.read_exact(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(&bytes, b"ok");
    gate.close().await.unwrap();
}

#[tokio::test]
async fn off_unsupported_identity_and_nonroot_fail_before_binding() {
    let mut input = request(8080);
    input.mode = ExposureMode::Off;
    assert!(matches!(
        Gate::bind(RESERVED_INGRESS_START, &input, Budgets::default()),
        Err(Error::Off)
    ));
    input.mode = ExposureMode::TailnetOnly;
    assert!(matches!(
        Gate::bind(RESERVED_INGRESS_START, &input, Budgets::default()),
        Err(Error::Policy(
            horizon_cloud_protocol::exposure::Error::IdentityUnavailable
        ))
    ));
    for source in [
        SourceRule::DnsName("host.test.ts.net".into()),
        SourceRule::Tag("tag:prod".into()),
    ] {
        input.mode = ExposureMode::Restricted;
        for sources in [
            vec![source.clone()],
            vec![SourceRule::Cidr("127.0.0.0/8".into()), source],
        ] {
            input.sources = sources;
            assert!(input.validate().is_ok(), "Valid syntax grants no worker support");
            assert!(matches!(
                Gate::bind(RESERVED_INGRESS_START, &input, Budgets::default()),
                Err(Error::Policy(
                    horizon_cloud_protocol::exposure::Error::IdentityUnavailable
                ))
            ));
        }
    }
    input = request(8080);
    if !rustix::process::geteuid().is_root() {
        assert!(matches!(
            Gate::bind(RESERVED_INGRESS_START, &input, Budgets::default()),
            Err(Error::Root)
        ));
    }
}

#[tokio::test]
async fn explicit_close_joins_tracked_flows_and_closes_listener_descriptors() {
    let server = backend().await;
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), Budgets::default());
    let shared = Arc::clone(&gate.shared);
    #[cfg(target_os = "linux")]
    let descriptors: Vec<_> = {
        use std::os::fd::AsRawFd;
        lock(&shared)
            .listeners
            .iter()
            .map(|listener| {
                let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", listener.as_raw_fd()));
                let socket = std::fs::read_link(&path).unwrap();
                (path, socket)
            })
            .collect()
    };
    let mut client = connect(address).await;
    let mut service = accepted(&server).await;
    assert_eq!(gate.counters().active_connections, 1);
    gate.close().await.unwrap();
    assert!(end(&mut client).await.is_empty());
    assert!(end(&mut service).await.is_empty());
    assert!(TcpStream::connect(address).await.is_err());
    assert_eq!(lock(&shared).counters().active_connections, 0);
    assert!(lock(&shared).listeners.is_empty());
    #[cfg(target_os = "linux")]
    for (path, socket) in descriptors {
        assert_ne!(std::fs::read_link(path).ok(), Some(socket));
    }
}

#[tokio::test]
async fn drop_fails_closed_with_active_streams() {
    let server = backend().await;
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), Budgets::default());
    let mut client = connect(address).await;
    let mut service = accepted(&server).await;
    drop(gate);
    assert!(end(&mut client).await.is_empty());
    assert!(end(&mut service).await.is_empty());
    timeout(WAIT, async {
        while TcpStream::connect(address).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn connection_slots_return_after_both_halves_end() {
    let server = backend().await;
    let mut input = request(server.local_addr().unwrap().port());
    input.limits = ConnectionLimits {
        total: 1,
        per_source: 1,
    };
    let (gate, address) = fixture(&input, Budgets::default());
    let first = connect(address).await;
    let service = accepted(&server).await;
    let mut second = connect(address).await;
    assert!(end(&mut second).await.is_empty());
    assert_eq!(gate.counters().active_connections, 1);
    drop(first);
    drop(service);
    inactive(&gate).await;
    let _third = connect(address).await;
    let _service = accepted(&server).await;
    assert_eq!(gate.counters().active_connections, 1);
    gate.close().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn per_source_and_global_connection_limits_are_independent() {
    let server = backend().await;
    let mut input = request(server.local_addr().unwrap().port());
    input.limits = ConnectionLimits {
        total: 2,
        per_source: 1,
    };
    let (gate, address) = fixture(&input, Budgets::default());
    let _first = connect(address).await;
    let _service1 = accepted(&server).await;
    let mut same = connect(address).await;
    assert!(end(&mut same).await.is_empty());
    let _second = connect_from(address, [127, 0, 0, 2]).await;
    let _service2 = accepted(&server).await;
    assert_eq!(gate.counters().active_connections, 2);
    let mut third = connect_from(address, [127, 0, 0, 3]).await;
    assert!(end(&mut third).await.is_empty());
    assert_eq!(gate.counters().refused, 2);
    gate.close().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn source_history_cap_refuses_new_peers_without_forgetting_prior_charges() {
    let server = backend().await;
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), Budgets::default());
    for last in 0..MAX_SOURCES {
        let client = connect_from(address, [127, 1, 0, u8::try_from(last).unwrap()]).await;
        let service = accepted(&server).await;
        drop((client, service));
        inactive(&gate).await;
    }
    assert_eq!(gate.counters().recorded_sources, MAX_SOURCES);
    let mut next = connect_from(address, [127, 2, 0, 1]).await;
    assert!(end(&mut next).await.is_empty());
    assert!(timeout(Duration::from_millis(100), server.accept()).await.is_err());
    let _known = connect_from(address, [127, 1, 0, 0]).await;
    let _service = accepted(&server).await;
    assert_eq!(gate.counters().recorded_sources, MAX_SOURCES);
    gate.close().await.unwrap();
}

#[tokio::test]
async fn byte_caps_apply_across_both_directions_and_keep_source_history() {
    let server = backend().await;
    let budgets = Budgets {
        connection_bytes: 4,
        source_bytes: 4,
        gate_bytes: 8,
        ..Budgets::default()
    };
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), budgets);
    let mut client = connect(address).await;
    let mut service = accepted(&server).await;
    client.write_all(b"ab").await.unwrap();
    let mut sent = [0; 2];
    timeout(WAIT, service.read_exact(&mut sent)).await.unwrap().unwrap();
    assert_eq!(&sent, b"ab");
    service.write_all(b"reply").await.unwrap();
    assert_eq!(end(&mut client).await, b"re");
    inactive(&gate).await;
    assert_eq!(gate.counters().charged_bytes, 4);
    assert!(gate.counters().forwarded_bytes <= 4);
    let mut again = connect(address).await;
    assert!(end(&mut again).await.is_empty());
    assert!(timeout(Duration::from_millis(100), server.accept()).await.is_err());
    gate.close().await.unwrap();
}

#[tokio::test]
async fn gate_budget_is_shared_by_distinct_actual_source_addresses() {
    let server = backend().await;
    let budgets = Budgets {
        connection_bytes: 4,
        source_bytes: 4,
        gate_bytes: 4,
        ..Budgets::default()
    };
    let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), budgets);
    let mut first = connect(address).await;
    let mut service = accepted(&server).await;
    first.write_all(b"1234").await.unwrap();
    assert_eq!(end(&mut service).await, b"1234");
    inactive(&gate).await;
    let socket = TcpSocket::new_v4().unwrap();
    socket
        .bind(("127.0.0.2".parse::<Ipv4Addr>().unwrap(), 0).into())
        .unwrap();
    let mut second = socket.connect(address).await.unwrap();
    assert!(end(&mut second).await.is_empty());
    assert_eq!(gate.counters().charged_bytes, 4);
    gate.close().await.unwrap();
}

#[tokio::test]
async fn idle_connection_and_monotonic_gate_lifetimes_close_real_streams() {
    for case in 0..3 {
        let server = backend().await;
        let short = Duration::from_millis(80);
        let mut budgets = Budgets::default();
        match case {
            0 => budgets.idle_timeout = short,
            1 => budgets.connection_lifetime = short,
            _ => budgets.gate_lifetime = short,
        }
        let (gate, address) = fixture(&request(server.local_addr().unwrap().port()), budgets);
        let mut client = connect(address).await;
        let mut service = accepted(&server).await;
        assert!(end(&mut client).await.is_empty());
        assert!(end(&mut service).await.is_empty());
        inactive(&gate).await;
        if case == 2 {
            assert!(TcpStream::connect(address).await.is_err());
            assert!(lock(&gate.shared).listeners.is_empty());
        }
        gate.close().await.unwrap();
    }
}

#[tokio::test]
async fn pending_admissions_count_toward_limits_and_cancel_before_the_first_poll() {
    let listener = backend().await;
    let _client = connect(listener.local_addr().unwrap()).await;
    let admission = Arc::new(accepted(&listener).await);
    let mut input = request(8080);
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
    let id = lock(&shared).admit(admission.clone()).unwrap();
    let _other = connect(listener.local_addr().unwrap()).await;
    let denied = Arc::new(accepted(&listener).await);
    assert!(lock(&shared).admit(denied).is_none());
    let (_cancel, watching) = watch::channel(false);
    let mut tasks = JoinSet::new();
    relay::spawn(&mut tasks, Arc::clone(&shared), id, admission, watching);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    assert_eq!(lock(&shared).counters().active_connections, 0);
}

#[test]
fn all_time_and_byte_budgets_have_finite_validated_ceilings() {
    assert_eq!(Budgets::default().gate_lifetime, Duration::from_secs(28800));
    for invalid in [
        Budgets {
            gate_lifetime: Duration::from_secs(28801),
            ..Budgets::default()
        },
        Budgets {
            gate_lifetime: Duration::ZERO,
            ..Budgets::default()
        },
        Budgets {
            idle_timeout: Duration::from_secs(61),
            ..Budgets::default()
        },
        Budgets {
            connect_timeout: Duration::from_secs(4),
            ..Budgets::default()
        },
        Budgets {
            connection_lifetime: Duration::from_secs(901),
            ..Budgets::default()
        },
        Budgets {
            connection_bytes: 0,
            ..Budgets::default()
        },
        Budgets {
            connection_bytes: u64::MAX,
            ..Budgets::default()
        },
    ] {
        assert!(matches!(invalid.validate(), Err(Error::Budgets)));
    }
}

#[tokio::test]
#[ignore = "Requires the approved task-owned root-worker component procedure"]
async fn actual_task_owned_worker_gate() {
    assert_eq!(std::env::var("HORIZON_GATE_COMPONENT").unwrap(), "task-owned-worker");
    let server = backend().await;
    let mut input = request(server.local_addr().unwrap().port());
    input.mode = ExposureMode::Restricted;
    input.sources = vec![SourceRule::Cidr(std::env::var("HORIZON_GATE_SOURCE_CIDR").unwrap())];
    let budgets = Budgets {
        gate_lifetime: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(10),
        ..Budgets::default()
    };
    let gate = Gate::bind(RESERVED_INGRESS_START, &input, budgets).unwrap();
    println!("horizon-gate-component-ready={RESERVED_INGRESS_START}");
    let mut service = timeout(Duration::from_secs(20), server.accept())
        .await
        .unwrap()
        .unwrap()
        .0;
    let mut bytes = [0; 4];
    timeout(WAIT, service.read_exact(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(&bytes, b"PING");
    service.write_all(b"PONG").await.unwrap();
    timeout(WAIT, service.read_exact(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(&bytes, b"DONE");
    assert_eq!(gate.counters().recorded_sources, 1);
    gate.close().await.unwrap();
    println!("horizon-gate-component-closed=1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_shutdown_before_watch_notification_is_orderly_cancellation() {
    let server = backend().await;
    let (mut gate, _) = fixture(&request(server.local_addr().unwrap().port()), Budgets::default());
    let task = gate.task.take().unwrap();
    // This is cancel's real ordering, with notification withheld until the
    // physically shut listener has made the accept loop finish.
    lock(&gate.shared).stop();
    timeout(WAIT, task).await.unwrap().unwrap().unwrap();
    assert!(lock(&gate.shared).listeners.is_empty());
    gate.close().await.unwrap();
}
