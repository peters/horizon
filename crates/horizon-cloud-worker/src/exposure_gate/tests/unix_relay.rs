//! Real socket transport fixtures only: these do not issue an `AcceptedPeer` or identity.
use super::*;
use crate::exposure_gate::{relay, tests::backend};
use horizon_cloud_protocol::exposure::{ConnectionLimits, ExposureMode, ExposureRequest};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    task::JoinHandle,
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(4);

fn state(target_port: u16, budgets: Budgets) -> Shared {
    let request = ExposureRequest {
        mode: ExposureMode::Public,
        target_port,
        sources: Vec::new(),
        limits: ConnectionLimits::default(),
    };
    Arc::new(Mutex::new(State::new(
        Vec::new(),
        request.validate().unwrap(),
        budgets.validate().unwrap(),
        Instant::now() + WAIT,
    )))
}

fn transport_fixture(shared: &Shared) -> (UnixStream, UnixStream, JoinHandle<()>, u64) {
    let (client, caller) = UnixStream::pair().unwrap();
    super::super::bounded_socket(&SockRef::from(&client)).unwrap();
    let (control, daemon) = UnixStream::pair().unwrap();
    let client = Arc::new(crate::trusted_tcp_producer::transport_fixture(client, control));
    // Exercise the common relay without fabricating WhoIs or an opaque peer.
    // This private test-only registration supplies no StableID or named authority.
    let id = lock(shared)
        .register(Client::Trusted(Arc::clone(&client)), SourceKey::Pending)
        .unwrap();
    let (cancel, watching) = watch::channel(false);
    let flow = relay::Flow {
        shared: Arc::clone(shared),
        id,
    };
    let task = tokio::spawn(async move {
        let _keep_cancel = cancel;
        relay::run(flow, client, watching).await;
    });
    (caller, daemon, task, id)
}

#[tokio::test]
async fn unix_request_half_close_preserves_tcp_response_and_releases_original_sockets() {
    let listener = backend().await;
    let shared = state(listener.local_addr().unwrap().port(), Budgets::default());
    let (mut caller, mut control, relay, _) = transport_fixture(&shared);
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    caller.write_all(b"request").await.unwrap();
    caller.shutdown().await.unwrap();
    let mut request = Vec::new();
    timeout(WAIT, service.read_to_end(&mut request)).await.unwrap().unwrap();
    assert_eq!(request, b"request");
    service.write_all(b"response").await.unwrap();
    service.shutdown().await.unwrap();
    let mut response = Vec::new();
    timeout(WAIT, caller.read_to_end(&mut response)).await.unwrap().unwrap();
    assert_eq!(response, b"response");
    assert!(!relay.is_finished(), "payload EOF alone cannot release control custody");
    control.write_all(&[0x01]).await.unwrap();
    drop(control);
    timeout(WAIT, relay).await.unwrap().unwrap();
    let state = lock(&shared);
    assert_eq!(state.counters().active_connections, 0);
    assert_eq!(state.counters().charged_bytes, 15);
    assert_eq!(state.counters().forwarded_bytes, 15);
    assert_eq!(state.verified_peers, 0);
}

#[tokio::test]
async fn unix_budget_response_drains_unread_reverse_input_before_original_flow_release() {
    let listener = backend().await;
    let shared = state(
        listener.local_addr().unwrap().port(),
        Budgets {
            connection_bytes: 8193,
            source_bytes: 8193,
            gate_bytes: 8193,
            ..Budgets::default()
        },
    );
    let (mut caller, mut control, relay, id) = transport_fixture(&shared);
    SockRef::from(&caller).set_recv_buffer_size(512).unwrap();
    let mut phase = lock(&shared).subscribe(id).unwrap();
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    caller.write_all(b"q").await.unwrap();
    let mut first = [0];
    timeout(WAIT, service.read_exact(&mut first)).await.unwrap().unwrap();
    assert_eq!(&first, b"q");
    service.write_all(&[9; 8192]).await.unwrap();
    service.shutdown().await.unwrap();
    timeout(WAIT, async {
        while *phase.borrow_and_update() == Phase::Running {
            phase.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    // The fixture acknowledges only after its payload has drained. It supplies no daemon identity.
    caller.write_all(&[7; 16 * 1024]).await.unwrap();
    caller.shutdown().await.unwrap();
    let mut response = Vec::new();
    timeout(WAIT, caller.read_to_end(&mut response)).await.unwrap().unwrap();
    assert_eq!(response, vec![9; 8192]);
    assert!(!relay.is_finished());
    control.write_all(&[0x01]).await.unwrap();
    drop(control);
    timeout(WAIT, relay).await.unwrap().unwrap();
    let mut extra = Vec::new();
    timeout(WAIT, service.read_to_end(&mut extra)).await.unwrap().unwrap();
    assert!(extra.is_empty(), "discarded input cannot exceed the forwarding budget");
    let state = lock(&shared);
    assert_eq!(state.counters().active_connections, 0);
    assert_eq!(state.counters().charged_bytes, 8193);
    assert_eq!(state.counters().forwarded_bytes, 8193);
    let (denied, _caller) = UnixStream::pair().unwrap();
    let denied = crate::trusted_tcp_producer::pending_fixture(denied);
    drop(state);
    assert!(lock(&shared).reserve_pending(&denied).is_none());
}

#[tokio::test]
async fn explicit_stop_physically_closes_retained_unix_socket_during_budget_grace() {
    let listener = backend().await;
    let shared = state(
        listener.local_addr().unwrap().port(),
        Budgets {
            connection_bytes: 1,
            source_bytes: 1,
            gate_bytes: 1,
            ..Budgets::default()
        },
    );
    let (mut caller, mut control, relay, _) = transport_fixture(&shared);
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    service.write_all(b"x").await.unwrap();
    let mut byte = [0];
    timeout(WAIT, caller.read_exact(&mut byte)).await.unwrap().unwrap();
    assert_eq!(&byte, b"x");
    assert_eq!(timeout(WAIT, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert!(!relay.is_finished(), "open reverse input retains the bounded grace");
    lock(&shared).stop();
    assert_eq!(timeout(WAIT, control.read(&mut byte)).await.unwrap().unwrap(), 0);
    timeout(WAIT, relay).await.unwrap().unwrap();
    assert_eq!(lock(&shared).counters().active_connections, 0);
    assert_eq!(lock(&shared).counters().charged_bytes, 1);
    assert_eq!(timeout(WAIT, service.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn global_budget_wakes_pending_unix_identity_and_releases_its_original_descriptor() {
    let listener = backend().await;
    let shared = state(
        listener.local_addr().unwrap().port(),
        Budgets {
            connection_bytes: 1,
            source_bytes: 1,
            gate_bytes: 1,
            ..Budgets::default()
        },
    );
    let (original, mut pending_peer) = UnixStream::pair().unwrap();
    let original = crate::trusted_tcp_producer::pending_fixture(original);
    let pending_id = lock(&shared).reserve_pending(&original).unwrap();
    assert_eq!(lock(&shared).reserve(pending_id, 1), 0);
    let mut direct_peer = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let direct = Arc::new(listener.accept().await.unwrap().0);
    let direct_id = lock(&shared).admit(Arc::clone(&direct)).unwrap();
    let (entered, identifying) = tokio::sync::oneshot::channel();
    let (_cancel, watching) = watch::channel(false);
    let flow = relay::Flow {
        shared: Arc::clone(&shared),
        id: pending_id,
    };
    let task = tokio::spawn(async move {
        let result = super::super::trusted::identify_pending(&flow, watching, async {
            entered.send(()).unwrap();
            std::future::pending().await
        })
        .await;
        assert!(result.is_none());
    });
    timeout(WAIT, identifying).await.unwrap().unwrap();
    assert_eq!(lock(&shared).reserve(direct_id, 1), 1);
    timeout(WAIT, direct.writable()).await.unwrap().unwrap();
    assert_eq!(direct.try_write(b"x").unwrap(), 1);
    lock(&shared).wrote(direct_id, 1);
    let mut byte = [0];
    timeout(WAIT, direct_peer.read_exact(&mut byte)).await.unwrap().unwrap();
    assert_eq!(&byte, b"x");
    timeout(WAIT, task).await.unwrap().unwrap();
    assert_eq!(timeout(WAIT, pending_peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    let state = lock(&shared);
    assert_eq!(state.counters().active_connections, 1);
    assert_eq!(state.counters().charged_bytes, 1);
    assert_eq!(state.counters().forwarded_bytes, 1);
    assert_eq!(state.verified_peers, 0);
    drop(state);
    drop(original);
    lock(&shared).release(direct_id);
}

#[tokio::test]
async fn missing_control_ack_cancels_a_flow_even_while_its_payload_peer_stays_open() {
    let listener = backend().await;
    let shared = state(listener.local_addr().unwrap().port(), Budgets::default());
    let (mut caller, control, relay, _) = transport_fixture(&shared);
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    drop(control);
    timeout(WAIT, relay).await.unwrap().unwrap();
    let mut byte = [0];
    assert_eq!(timeout(WAIT, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(timeout(WAIT, service.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(lock(&shared).counters().active_connections, 0);
}

#[tokio::test]
async fn budget_drain_observes_control_failure_without_waiting_for_open_reverse_input() {
    let listener = backend().await;
    let shared = state(
        listener.local_addr().unwrap().port(),
        Budgets {
            connection_bytes: 1,
            source_bytes: 1,
            gate_bytes: 1,
            ..Budgets::default()
        },
    );
    let (mut caller, control, relay, id) = transport_fixture(&shared);
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    service.write_all(b"x").await.unwrap();
    let mut byte = [0];
    timeout(WAIT, caller.read_exact(&mut byte)).await.unwrap().unwrap();
    assert_eq!(&byte, b"x");
    assert_eq!(timeout(WAIT, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert!(matches!(lock(&shared).phase(id), Phase::Budget(_)));
    drop(control);
    // Existing grace is three seconds. This assertion requires control failure
    // to wake the owner while the original reverse payload remains open.
    timeout(Duration::from_secs(1), relay).await.unwrap().unwrap();
    assert_eq!(lock(&shared).counters().active_connections, 0);
    assert_eq!(timeout(WAIT, service.read(&mut byte)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn slow_payload_reader_receives_large_response_before_control_ack_releases_flow() {
    let listener = backend().await;
    let shared = state(listener.local_addr().unwrap().port(), Budgets::default());
    let (mut caller, mut control, relay, _) = transport_fixture(&shared);
    SockRef::from(&caller).set_recv_buffer_size(512).unwrap();
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    caller.write_all(b"q").await.unwrap();
    caller.shutdown().await.unwrap();
    let service = tokio::spawn(async move {
        let mut request = Vec::new();
        service.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"q");
        let response = vec![0x52; 64 * 1024].into_boxed_slice();
        service.write_all(&response).await.unwrap();
        service.shutdown().await.unwrap();
    });
    let mut count = 0;
    let mut bytes = [0; 257];
    timeout(WAIT, async {
        loop {
            let received = caller.read(&mut bytes).await.unwrap();
            if received == 0 {
                break;
            }
            assert!(bytes[..received].iter().all(|byte| *byte == 0x52));
            count += received;
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(count, 64 * 1024);
    timeout(WAIT, service).await.unwrap().unwrap();
    assert!(!relay.is_finished());
    assert_eq!(lock(&shared).counters().active_connections, 1);
    control.write_all(&[0x01]).await.unwrap();
    drop(control);
    timeout(WAIT, relay).await.unwrap().unwrap();
    assert_eq!(lock(&shared).counters().active_connections, 0);
    assert_eq!(lock(&shared).counters().forwarded_bytes, 64 * 1024 + 1);
}
