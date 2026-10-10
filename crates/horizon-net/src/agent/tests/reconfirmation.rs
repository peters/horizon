use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn restarted_grant_requires_authenticated_confirmation_before_backend_traffic() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), reconfirmation())
        .await
        .map_err(|_| Error::Timeout)?
}

async fn reconfirmation() -> Result<()> {
    let (relay, relay_url) = test_relay().await?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let (mut config, mut topology, _) = withdrawal_fixture(&relay_url);
    topology.services.insert(
        "worker/echo".into(),
        Service {
            node: "worker".into(),
            port: listener.local_addr()?.port(),
        },
    );
    topology.grants.insert(
        "lease".into(),
        Grant {
            from: vec!["authority".into()],
            to: "worker/echo".into(),
            expires_at: unix_now() + 60,
        },
    );
    config.topology = topology.clone();
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().join("worker");
    let first = Agent::bind_with_store(config.clone(), directory.clone()).await?;
    assert_eq!(first.controller().status().policy_state, crate::PolicyState::Confirmed);
    first.shutdown().await;
    drop(first);
    let worker = Agent::bind_with_store(config.clone(), directory).await?;
    let authority = Agent::bind(
        AgentConfig {
            node: "authority".into(),
            secret_key: "15".repeat(32),
            ..config
        },
        Controller::new(topology.clone())?,
    )
    .await?;
    worker.online().await?;
    authority.online().await?;
    let outsider = assert_quarantined_service(&worker, &authority, &listener, &topology, &relay_url).await?;
    authority.push_topology(worker.addr(), topology.clone()).await?;
    assert_eq!(worker.controller().status().policy_state, crate::PolicyState::Confirmed);
    let echo = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = [0; 9];
        socket.read_exact(&mut bytes).await?;
        socket.write_all(&bytes).await?;
        Ok::<(), std::io::Error>(())
    });
    let forwarder = authority.forward(worker.addr(), "worker/echo".into(), 0).await?;
    let mut socket = TcpStream::connect(forwarder.address()).await?;
    socket.write_all(b"confirmed").await?;
    let mut bytes = [0; 9];
    socket.read_exact(&mut bytes).await?;
    assert_eq!(&bytes, b"confirmed");
    tokio::time::timeout(Duration::from_secs(2), echo)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)??;
    drop(socket);
    drop(forwarder);
    outsider.close().await;
    authority.shutdown().await;
    worker.shutdown().await;
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}

async fn assert_quarantined_service(
    worker: &Agent,
    authority: &Agent,
    listener: &TcpListener,
    topology: &Topology,
    relay_url: &str,
) -> Result<Endpoint> {
    assert_eq!(
        worker.controller().status().policy_state,
        crate::PolicyState::AwaitingConfirmation
    );
    let stale_connection = authority
        .endpoint
        .connect(worker.addr(), wire::ALPN)
        .await
        .map_err(transport)?;
    assert!(matches!(
        worker.controller().register(
            authority.id(),
            worker.id(),
            "worker/echo",
            listener.local_addr()?.port(),
            stale_connection.clone()
        ),
        Err(Error::Denied)
    ));
    assert_eq!(worker.controller().status().sessions, 0);
    stale_connection.close(0_u32.into(), b"quarantined registration denied");
    assert!(
        !request_accepted(
            &authority.endpoint,
            worker.addr(),
            Request::Connect {
                network: topology.network.clone(),
                service: "worker/echo".into()
            }
        )
        .await?
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    let outsider = Endpoint::builder(Minimal)
        .relay_mode(RelayMode::custom([relay_url.parse().map_err(transport)?]))
        .clear_ip_transports()
        .bind()
        .await
        .map_err(transport)?;
    outsider.online().await;
    assert!(
        !request_accepted(
            &outsider,
            worker.addr(),
            Request::Update {
                topology: topology.clone()
            }
        )
        .await?
    );
    assert_eq!(
        worker.controller().status().policy_state,
        crate::PolicyState::AwaitingConfirmation
    );
    Ok(outsider)
}
