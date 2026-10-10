use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn unsupported_persistence_preserves_live_in_memory_transport_and_policy() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), memory_transport())
        .await
        .map_err(|_| Error::Timeout)?
}

async fn memory_transport() -> Result<()> {
    let (relay, relay_url) = test_relay().await?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let (mut config, mut topology, authority_key) = withdrawal_fixture(&relay_url);
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
    let controller = Controller::new(topology.clone())?;
    let worker = Agent::bind(config.clone(), controller.clone()).await?;
    let authority = Agent::bind(
        AgentConfig {
            node: "authority".into(),
            secret_key: "15".repeat(32),
            ..config.clone()
        },
        Controller::new(topology.clone())?,
    )
    .await?;
    assert_eq!(authority.addr().id, authority_key.public());
    worker.online().await?;
    authority.online().await?;
    let echo = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let (mut read, mut write) = socket.split();
        let _ = tokio::io::copy(&mut read, &mut write).await;
        Ok::<(), std::io::Error>(())
    });
    let forwarder = authority.forward(worker.addr(), "worker/echo".into(), 0).await?;
    let mut socket = TcpStream::connect(forwarder.address()).await?;
    socket.write_all(b"live").await?;
    let mut bytes = [0; 4];
    socket.read_exact(&mut bytes).await?;
    assert_eq!(&bytes, b"live");
    assert_eq!(controller.status().sessions, 1);

    let temporary = tempfile::tempdir()?;
    for existing in [false, true] {
        let enrollment = temporary
            .path()
            .join(if existing { "existing.json" } else { "absent.json" });
        let state = enrollment.with_extension("state");
        let original = serde_json::to_vec(&config)?;
        std::fs::write(&enrollment, &original)?;
        if existing {
            std::fs::create_dir(&state)?;
            std::fs::write(state.join("agent.lock"), b"retained owner")?;
            std::fs::write(state.join("00000000000000000001.json"), b"retained malformed state")?;
        }
        assert!(matches!(
            Agent::bind_with_store(config.clone(), state.clone()).await,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported
        ));
        assert!(matches!(
            Agent::bind_persistent(&enrollment).await,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported
        ));
        assert_eq!(std::fs::read(&enrollment)?, original);
        if existing {
            assert_eq!(std::fs::read_dir(&state)?.count(), 2);
            assert_eq!(std::fs::read(state.join("agent.lock"))?, b"retained owner");
            assert_eq!(
                std::fs::read(state.join("00000000000000000001.json"))?,
                b"retained malformed state"
            );
        } else {
            assert!(!state.exists());
        }
        assert_eq!(controller.topology(), topology);
        assert_eq!(controller.status().sessions, 1);
        socket.write_all(b"kept").await?;
        socket.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"kept");
    }

    let mut proposed = topology.clone();
    proposed.revision = 1;
    proposed.grants.clear();
    assert!(matches!(
        authority.push_topology(worker.addr(), proposed).await,
        Err(Error::Denied)
    ));
    assert_eq!(controller.topology(), topology);
    assert!(controller.revoke("lease")?);
    assert_eq!(controller.topology().revision, 1);
    assert_eq!(controller.status().sessions, 0);
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
        .await
        .map_err(|_| Error::Timeout)?;
    assert!(matches!(closed, Err(_) | Ok(0)));
    drop(socket);
    drop(forwarder);
    authority.shutdown().await;
    worker.shutdown().await;
    tokio::time::timeout(Duration::from_secs(2), echo)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)??;
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}
