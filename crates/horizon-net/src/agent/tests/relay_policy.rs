use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn caller_addresses_cannot_introduce_an_unconfigured_relay() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), relay_policy_scenario())
        .await
        .map_err(|_| Error::Timeout)?
}

async fn relay_policy_scenario() -> Result<()> {
    let (relay, relay_url) = test_relay().await?;
    let echo = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let unconfigured = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let foreign: iroh::RelayUrl = format!("http://{}", unconfigured.local_addr()?)
        .parse()
        .map_err(transport)?;
    let source_key = SecretKey::from_bytes(&[51; 32]);
    let target_key = SecretKey::from_bytes(&[52; 32]);
    let mut topology = Topology::empty("relay-policy-island");
    for (name, key) in [("source", &source_key), ("target", &target_key)] {
        topology.nodes.insert(
            name.into(),
            Node {
                key: key.public().to_string(),
            },
        );
    }
    topology.services.insert(
        "target/echo".into(),
        Service {
            node: "target".into(),
            port: echo.local_addr()?.port(),
        },
    );
    topology.grants.insert(
        "echo-lease".into(),
        Grant {
            from: vec!["source".into()],
            to: "target/echo".into(),
            expires_at: unix_now() + 60,
        },
    );
    let config = |name: &str, byte: &str| AgentConfig {
        node: name.into(),
        secret_key: byte.repeat(32),
        authority_key: source_key.public().to_string(),
        topology: topology.clone(),
        relay_urls: vec![relay_url.clone()],
        relay_only: true,
    };
    let source = Agent::bind(config("source", "33"), Controller::new(topology.clone())?).await?;
    let target = Agent::bind(config("target", "34"), Controller::new(topology.clone())?).await?;
    source.online().await?;
    target.online().await?;
    source.probe(target.addr()).await?;
    let mut update = topology;
    update.revision = 1;
    for destination in [
        EndpointAddr::new(target.id()).with_relay_url(foreign.clone()),
        target.addr().with_relay_url(foreign),
    ] {
        assert!(matches!(source.probe(destination.clone()).await, Err(Error::Denied)));
        assert!(matches!(
            source.push_topology(destination.clone(), update.clone()).await,
            Err(Error::Denied)
        ));
        assert!(matches!(
            source.forward(destination, "target/echo".into(), 0).await,
            Err(Error::Denied)
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), unconfigured.accept())
            .await
            .is_err()
    );
    let forward = source.forward(target.addr(), "target/echo".into(), 0).await?;
    let echo_task = tokio::spawn(async move {
        let (mut socket, _) = echo.accept().await?;
        let (mut read, mut write) = socket.split();
        tokio::io::copy(&mut read, &mut write).await?;
        Result::Ok(())
    });
    let mut socket = TcpStream::connect(forward.address()).await?;
    socket.write_all(b"configured relay bytes").await?;
    let mut received = [0; 22];
    socket.read_exact(&mut received).await?;
    assert_eq!(&received, b"configured relay bytes");
    drop(socket);
    drop(forward);
    source.close().await;
    target.close().await;
    echo_task.await.map_err(transport)??;
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}
