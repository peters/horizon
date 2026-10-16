use super::super::{Budgets, State};
use super::*;
use horizon_cloud_protocol::exposure::{ConnectionLimits, ExposureMode, ExposureRequest};
use std::sync::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, timeout},
};

const WAIT: Duration = Duration::from_secs(4);

async fn pair(source: [u8; 4]) -> (Arc<TcpStream>, TcpStream) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind((Ipv4Addr::from(source), 0).into()).unwrap();
    let peer = socket.connect(listener.local_addr().unwrap()).await.unwrap();
    let stream = listener.accept().await.unwrap().0;
    (Arc::new(stream), peer)
}
fn state(budgets: Budgets) -> Shared {
    let request = ExposureRequest {
        mode: ExposureMode::Public,
        target_port: 8080,
        sources: Vec::new(),
        limits: ConnectionLimits::default(),
    };
    Arc::new(Mutex::new(State::new(
        Vec::new(),
        request.validate().unwrap(),
        budgets,
        Instant::now() + WAIT,
    )))
}
async fn flow(shared: &Shared, source: [u8; 4]) -> (Flow, Arc<TcpStream>, TcpStream, Arc<TcpStream>, TcpStream) {
    let (client, caller) = pair(source).await;
    let (backend, service) = pair([127, 0, 0, 1]).await;
    let id = lock(shared).admit(Arc::clone(&client)).unwrap();
    assert!(lock(shared).attach(id, Arc::clone(&backend)));
    (
        Flow {
            shared: Arc::clone(shared),
            id,
        },
        client,
        caller,
        backend,
        service,
    )
}
async fn write_part(flow: &Flow, to: &TcpStream, bytes: &[u8]) {
    timeout(WAIT, to.writable()).await.unwrap().unwrap();
    let count = to.try_write(bytes).unwrap();
    assert_eq!(count, bytes.len());
    lock(&flow.shared).wrote(flow.id, count);
}
async fn read(peer: &mut TcpStream, expected: &[u8]) {
    let mut bytes = vec![0; expected.len()];
    timeout(WAIT, peer.read_exact(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(bytes, expected);
}

#[tokio::test]
async fn final_reservation_drains_each_partial_write_before_budget_shutdown() {
    let shared = state(Budgets {
        connection_bytes: 8,
        source_bytes: 8,
        gate_bytes: 8,
        ..Budgets::default()
    });
    let (flow, _, _, backend, mut service) = flow(&shared, [127, 0, 0, 1]).await;
    assert_eq!(lock(&shared).reserve(flow.id, 12), 8);
    for (index, bytes) in [b"ab", b"cd", b"ef", b"gh"].into_iter().enumerate() {
        write_part(&flow, &backend, bytes).await;
        read(&mut service, bytes).await;
        assert_eq!(lock(&shared).deadline(flow.id).is_some(), index < 3);
    }
    let counters = lock(&shared).counters();
    assert_eq!(counters.charged_bytes, 8);
    assert_eq!(counters.forwarded_bytes, 8);
    let (client, _) = pair([127, 0, 0, 1]).await;
    assert!(lock(&shared).admit(client).is_none());
}

#[tokio::test]
async fn concurrent_source_and_gate_reservations_do_not_cancel_pending_siblings() {
    for source_limit in [true, false] {
        let budgets = Budgets {
            connection_bytes: 8,
            source_bytes: 8,
            gate_bytes: if source_limit { 16 } else { 8 },
            ..Budgets::default()
        };
        let shared = state(budgets);
        let (first, _, _, first_backend, mut first_service) = flow(&shared, [127, 0, 0, 1]).await;
        let second_source = if source_limit { [127, 0, 0, 1] } else { [127, 0, 0, 2] };
        let (second, _, _, second_backend, mut second_service) = flow(&shared, second_source).await;
        let (unrelated, _, _, _, _) = flow(&shared, [127, 0, 0, 3]).await;
        assert_eq!(lock(&shared).reserve(first.id, 4), 4);
        assert_eq!(lock(&shared).reserve(second.id, 4), 4);
        write_part(&second, &second_backend, b"last").await;
        read(&mut second_service, b"last").await;
        assert!(lock(&shared).deadline(second.id).is_none());
        assert!(lock(&shared).deadline(first.id).is_some());
        assert_eq!(lock(&shared).deadline(unrelated.id).is_some(), source_limit);
        assert_eq!(lock(&shared).reserve(first.id, 1), 0);
        assert!(lock(&shared).deadline(first.id).is_some());
        write_part(&first, &first_backend, b"fi").await;
        assert!(lock(&shared).deadline(first.id).is_some());
        write_part(&first, &first_backend, b"rs").await;
        read(&mut first_service, b"firs").await;
        assert!(lock(&shared).deadline(first.id).is_none());
        assert_eq!(lock(&shared).counters().charged_bytes, 8);
        assert_eq!(lock(&shared).counters().forwarded_bytes, 8);
        let (client, _) = pair([127, 0, 0, 1]).await;
        assert!(lock(&shared).admit(client).is_none());
    }
}

#[tokio::test]
async fn partial_and_zero_allowance_return_without_cancelling_opposite_direction() {
    for remaining in [0, 2] {
        let shared = state(Budgets {
            connection_bytes: 4,
            source_bytes: 4,
            gate_bytes: 4,
            ..Budgets::default()
        });
        let (flow, client, mut caller, backend, mut service) = flow(&shared, [127, 0, 0, 1]).await;
        let opposite = 4 - remaining;
        assert_eq!(lock(&shared).reserve(flow.id, opposite), opposite);
        caller.write_all(b"abcd").await.unwrap();
        let (_cancel, watching) = watch::channel(false);
        timeout(WAIT, copy(&flow, Arc::clone(&client), backend, watching))
            .await
            .unwrap();
        assert!(lock(&shared).deadline(flow.id).is_some());
        read(&mut service, &b"ab"[..remaining]).await;
        assert_eq!(lock(&shared).counters().forwarded_bytes, remaining as u64);
        write_part(&flow, &client, &b"reply"[..opposite]).await;
        read(&mut caller, &b"reply"[..opposite]).await;
        assert!(lock(&shared).deadline(flow.id).is_none());
        assert_eq!(lock(&shared).counters().charged_bytes, 4);
        assert_eq!(lock(&shared).counters().forwarded_bytes, 4);
    }
}

#[tokio::test]
async fn cancellation_and_deadline_discard_pending_bytes_without_refunding_charge() {
    for cancel in [true, false] {
        let shared = state(Budgets {
            connection_bytes: 8,
            source_bytes: 8,
            gate_bytes: 8,
            ..Budgets::default()
        });
        let (flow, _, mut caller, _, mut service) = flow(&shared, [127, 0, 0, 1]).await;
        assert_eq!(lock(&shared).reserve(flow.id, 8), 8);
        if cancel {
            lock(&shared).stop();
        } else {
            lock(&shared).until = Instant::now();
            assert_eq!(lock(&shared).reserve(flow.id, 1), 0);
        }
        assert!(lock(&shared).deadline(flow.id).is_none());
        let mut byte = [0];
        assert_eq!(timeout(WAIT, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
        assert_eq!(timeout(WAIT, service.read(&mut byte)).await.unwrap().unwrap(), 0);
        assert_eq!(lock(&shared).counters().charged_bytes, 8);
        assert_eq!(lock(&shared).counters().forwarded_bytes, 0);
    }
}
