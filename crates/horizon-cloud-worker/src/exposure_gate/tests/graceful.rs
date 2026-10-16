use super::*;
use crate::exposure_gate::tests::backend;

#[tokio::test]
async fn slow_reader_receives_final_budget_bytes_after_unread_request_and_flow_release() {
    slow_reader(false).await;
}

#[tokio::test]
async fn completed_copy_join_still_enters_budget_drain_before_flow_release() {
    slow_reader(true).await;
}

async fn slow_reader(join_completes: bool) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let socket = TcpSocket::new_v4().unwrap();
    SockRef::from(&socket).set_recv_buffer_size(512).unwrap();
    let mut caller = socket.connect(listener.local_addr().unwrap()).await.unwrap();
    let client = Arc::new(listener.accept().await.unwrap().0);
    let (backend, mut service) = pair([127, 0, 0, 1]).await;
    let shared = state(Budgets {
        connection_bytes: 8192,
        source_bytes: 8192,
        gate_bytes: 8192,
        ..Budgets::default()
    });
    let id = lock(&shared).admit(Arc::clone(&client)).unwrap();
    assert!(lock(&shared).attach(id, Arc::clone(&backend)));
    let flow = Flow {
        shared: Arc::clone(&shared),
        id,
    };
    caller.write_all(&[7; 16 * 1024]).await.unwrap();
    caller.shutdown().await.unwrap();
    service.write_all(&[9; 8192]).await.unwrap();
    service.shutdown().await.unwrap();
    let (_cancel, watching) = watch::channel(false);
    let forwarding = async {
        // Deliberately leave reverse input unread until the production budget
        // coordinator takes ownership; no synthetic write counters are used.
        let end = copy(&flow, Arc::clone(&backend), Arc::clone(&client), watching.clone()).await;
        if !join_completes {
            pending::<()>().await;
        }
        (end, CopyEnd::Eof)
    };
    timeout(
        WAIT,
        coordinate(&flow, &client, Some(&backend), watching.clone(), forwarding),
    )
    .await
    .unwrap();
    drop(flow);
    drop(client);
    drop(backend);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut bytes = Vec::new();
    let result = timeout(WAIT, caller.read_to_end(&mut bytes)).await.unwrap();
    assert_eq!(bytes.len(), 8192, "actual peer bytes, read result={result:?}");
    assert!(result.is_ok(), "clean EOF required: {result:?}");
    assert_eq!(bytes, vec![9; 8192]);
    let counters = lock(&shared).counters();
    assert_eq!(counters.active_connections, 0);
    assert_eq!(counters.charged_bytes, 8192);
    assert_eq!(counters.forwarded_bytes, 8192);
}

#[tokio::test]
async fn full_relay_budget_drains_reverse_input_without_forwarding_it() {
    let listener = backend().await;
    let socket = TcpSocket::new_v4().unwrap();
    SockRef::from(&socket).set_recv_buffer_size(512).unwrap();
    let incoming = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut caller = socket.connect(incoming.local_addr().unwrap()).await.unwrap();
    let client = Arc::new(incoming.accept().await.unwrap().0);
    let shared = state_for_target(
        Budgets {
            connection_bytes: 8193,
            source_bytes: 8193,
            gate_bytes: 8193,
            ..Budgets::default()
        },
        listener.local_addr().unwrap().port(),
    );
    let id = lock(&shared).admit(Arc::clone(&client)).unwrap();
    let mut phase = lock(&shared).subscribe(id).unwrap();
    let (_cancel, watching) = watch::channel(false);
    let relay = tokio::spawn(run(
        Flow {
            shared: Arc::clone(&shared),
            id,
        },
        client,
        watching,
    ));
    let mut service = timeout(WAIT, listener.accept()).await.unwrap().unwrap().0;
    caller.write_all(b"q").await.unwrap();
    read(&mut service, b"q").await;
    service.write_all(&[9; 8192]).await.unwrap();
    service.shutdown().await.unwrap();
    timeout(WAIT, async {
        while *phase.borrow_and_update() == Phase::Running {
            phase.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    caller.write_all(&[7; 16 * 1024]).await.unwrap();
    caller.shutdown().await.unwrap();
    timeout(WAIT, relay).await.unwrap().unwrap();
    assert_eq!(lock(&shared).counters().active_connections, 0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut bytes = Vec::new();
    timeout(WAIT, caller.read_to_end(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(bytes, vec![9; 8192]);
    let mut extra = Vec::new();
    timeout(WAIT, service.read_to_end(&mut extra)).await.unwrap().unwrap();
    assert!(extra.is_empty());
    let counters = lock(&shared).counters();
    assert_eq!(counters.charged_bytes, 8193);
    assert_eq!(counters.forwarded_bytes, 8193);
}

#[tokio::test]
async fn global_budget_wakes_idle_sibling_and_explicit_cancel_overrides_grace() {
    let shared = state(Budgets {
        gate_bytes: 1,
        ..Budgets::default()
    });
    let (idle, client, mut caller, backend, mut service) = flow(&shared, [127, 0, 0, 1]).await;
    let idle_id = idle.id;
    let (charging, _, _charging_caller, charging_backend, _charging_service) = flow(&shared, [127, 0, 0, 2]).await;
    let (_cancel, watching) = watch::channel(false);
    let relay = tokio::spawn(async move {
        let forwarding = async {
            tokio::join!(
                copy(&idle, Arc::clone(&client), Arc::clone(&backend), watching.clone()),
                copy(&idle, Arc::clone(&backend), Arc::clone(&client), watching.clone()),
            )
        };
        coordinate(&idle, &client, Some(&backend), watching.clone(), forwarding).await;
    });
    tokio::task::yield_now().await;
    assert_eq!(lock(&shared).reserve(charging.id, 1), 1);
    write_part(&charging, &charging_backend, b"x").await;
    let mut byte = [0];
    let prompt = Duration::from_millis(500);
    assert_eq!(timeout(prompt, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert_eq!(timeout(prompt, service.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert!(!relay.is_finished());
    lock(&shared).cancel_flow(idle_id);
    timeout(prompt, relay).await.unwrap().unwrap();
    assert_eq!(lock(&shared).phase(idle_id), Phase::Cancelled);
    let counters = lock(&shared).counters();
    assert_eq!(counters.charged_bytes, 1);
    assert_eq!(counters.forwarded_bytes, 1);
    assert_eq!(counters.active_connections, 1);
    let (client, _) = pair([127, 0, 0, 3]).await;
    assert!(lock(&shared).admit(client).is_none());
}

#[tokio::test]
async fn discard_cap_cannot_extend_original_deadline_or_refund_budget() {
    let shared = state(Budgets {
        gate_bytes: 1,
        ..Budgets::default()
    });
    let (flow, client, mut caller, backend, mut service) = flow(&shared, [127, 0, 0, 1]).await;
    lock(&shared).until = Instant::now() + Duration::from_millis(150);
    service.write_all(b"x").await.unwrap();
    service.shutdown().await.unwrap();
    let (_cancel, watching) = watch::channel(false);
    let started = Instant::now();
    let relay = tokio::spawn(async move {
        let forwarding = async {
            let end = copy(&flow, Arc::clone(&backend), Arc::clone(&client), watching.clone()).await;
            (end, CopyEnd::Eof)
        };
        coordinate(&flow, &client, Some(&backend), watching.clone(), forwarding).await;
    });
    read(&mut caller, b"x").await;
    caller.write_all(&vec![7; 64 * 1024 + 1]).await.unwrap();
    timeout(Duration::from_millis(500), relay).await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    let counters = lock(&shared).counters();
    assert_eq!(counters.active_connections, 0);
    assert_eq!(counters.charged_bytes, 1);
    assert_eq!(counters.forwarded_bytes, 1);
}

#[tokio::test]
async fn already_budgeted_flow_drains_client_without_dialling_backend() {
    let listener = backend().await;
    let shared = state_for_target(
        Budgets {
            gate_bytes: 1,
            ..Budgets::default()
        },
        listener.local_addr().unwrap().port(),
    );
    let (client, mut caller) = pair([127, 0, 0, 1]).await;
    let id = lock(&shared).admit(Arc::clone(&client)).unwrap();
    let (charging, _, _charging_caller, backend, _charging_service) = flow(&shared, [127, 0, 0, 2]).await;
    assert_eq!(lock(&shared).reserve(charging.id, 1), 1);
    write_part(&charging, &backend, b"x").await;
    caller.shutdown().await.unwrap();
    let (_cancel, watching) = watch::channel(false);
    timeout(
        WAIT,
        run(
            Flow {
                shared: Arc::clone(&shared),
                id,
            },
            client,
            watching,
        ),
    )
    .await
    .unwrap();
    let mut byte = [0];
    assert_eq!(timeout(WAIT, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    assert!(timeout(Duration::from_millis(50), listener.accept()).await.is_err());
    assert_eq!(lock(&shared).counters().active_connections, 1);
}

#[tokio::test]
async fn peer_reset_during_budget_grace_closes_the_other_socket_promptly() {
    let shared = state(Budgets {
        gate_bytes: 1,
        ..Budgets::default()
    });
    let (flow, client, mut caller, backend, mut service) = flow(&shared, [127, 0, 0, 1]).await;
    assert_eq!(lock(&shared).reserve(flow.id, 1), 1);
    write_part(&flow, &client, b"x").await;
    let (_cancel, watching) = watch::channel(false);
    let relay = tokio::spawn(async move {
        coordinate(&flow, &client, Some(&backend), watching, pending()).await;
    });
    read(&mut caller, b"x").await;
    let mut byte = [0];
    let prompt = Duration::from_millis(500);
    assert_eq!(timeout(prompt, caller.read(&mut byte)).await.unwrap().unwrap(), 0);
    SockRef::from(&caller).set_linger(Some(Duration::ZERO)).unwrap();
    drop(caller);
    timeout(prompt, relay).await.unwrap().unwrap();
    assert_eq!(timeout(prompt, service.read(&mut byte)).await.unwrap().unwrap(), 0);
    let counters = lock(&shared).counters();
    assert_eq!(counters.active_connections, 0);
    assert_eq!(counters.charged_bytes, 1);
    assert_eq!(counters.forwarded_bytes, 1);
}
