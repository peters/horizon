use std::{
    net::Ipv4Addr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use horizon_net::{Agent, AgentConfig, Controller, Error, Grant, Node, SecretKey, Service, Topology};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

async fn bind_agent(
    secret: &SecretKey,
    name: &str,
    topology: &Topology,
    relay: &str,
    state: &std::path::Path,
) -> Result<(Agent, Controller), Error> {
    let config = AgentConfig {
        node: name.into(),
        secret_key: secret_hex(secret),
        authority_key: topology.nodes["authority"].key.clone(),
        topology: topology.clone(),
        relay_urls: vec![relay.into()],
        relay_only: true,
    };
    let agent = Agent::bind_with_store(config, state.join(name)).await?;
    let controller = agent.controller();
    agent.online().await?;
    Ok((agent, controller))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_only_relay_forwards_and_revocation_closes_live_socket() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(45), scenario(Termination::Revoke))
        .await
        .map_err(|_| Error::Timeout)?
}

#[derive(Clone, Copy)]
enum Termination {
    Revoke,
    Expire,
    Rotate,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_expiry_closes_live_socket_without_authority() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(45), scenario(Termination::Expire))
        .await
        .map_err(|_| Error::Timeout)?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_rotation_closes_live_socket_and_rejects_old_identity() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(45), scenario(Termination::Rotate))
        .await
        .map_err(|_| Error::Timeout)?
}

async fn scenario(termination: Termination) -> Result<(), Error> {
    let state = tempfile::tempdir()?;
    let (relay, relay_url) = start_relay().await?;
    let (echo_port, echo) = start_echo().await?;
    let worker_key = SecretKey::from_bytes(&[1; 32]);
    let mac_key = SecretKey::from_bytes(&[2; 32]);
    let authority_key = SecretKey::from_bytes(&[3; 32]);
    let mut topology = Topology::empty("synthetic-island");
    for (name, key) in [
        ("worker", &worker_key),
        ("mac", &mac_key),
        ("authority", &authority_key),
    ] {
        topology.nodes.insert(
            name.into(),
            Node {
                key: key.public().to_string(),
            },
        );
    }
    topology.services.insert(
        "mac/echo".into(),
        Service {
            node: "mac".into(),
            port: echo_port,
        },
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(transport)?
        .as_secs();
    topology.grants.insert(
        "worker-echo".into(),
        Grant {
            from: vec!["worker".into()],
            to: "mac/echo".into(),
            expires_at: now + 60,
        },
    );
    let (mac, mac_controller) = bind_agent(&mac_key, "mac", &topology, &relay_url, state.path()).await?;
    let (worker, worker_controller) = bind_agent(&worker_key, "worker", &topology, &relay_url, state.path()).await?;
    let (authority, _) = bind_agent(&authority_key, "authority", &topology, &relay_url, state.path()).await?;
    let forwarder = worker.forward(mac.addr(), "mac/echo".into(), 0).await?;
    let mut socket = TcpStream::connect(forwarder.address()).await?;
    socket.write_all(b"encrypted relay bytes").await?;
    let mut bytes = [0; 21];
    socket.read_exact(&mut bytes).await?;
    assert_eq!(&bytes, b"encrypted relay bytes");
    assert_eq!(mac_controller.status().sessions, 1);
    assert_eq!(worker_controller.status().sessions, 1);
    // A node's valid QUIC identity is not topology authority.
    let mut unauthorized = topology.clone();
    unauthorized.revision = 1;
    unauthorized.grants.clear();
    assert!(matches!(
        worker.push_topology(mac.addr(), unauthorized.clone()).await,
        Err(Error::Denied)
    ));
    assert_eq!(mac_controller.topology(), topology);
    apply_termination(termination, &topology, &mut unauthorized)?;
    authority.push_topology(mac.addr(), unauthorized).await?;
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
        .await
        .map_err(|_| Error::Timeout)?;
    assert!(matches!(closed, Err(_) | Ok(0)));
    assert_eq!(mac_controller.status().sessions, 0);
    let mut denied = TcpStream::connect(forwarder.address()).await?;
    denied.write_all(b"must never reach echo").await?;
    let closed = tokio::time::timeout(Duration::from_secs(2), denied.read(&mut byte))
        .await
        .map_err(|_| Error::Timeout)?;
    assert!(matches!(closed, Err(_) | Ok(0)));
    drop(forwarder);
    authority.close().await;
    worker.close().await;
    mac.close().await;
    drop(mac_controller);
    let (mac, controller) = bind_agent(&mac_key, "mac", &topology, &relay_url, state.path()).await?;
    assert_eq!(controller.topology().revision, 1);
    assert!(
        controller
            .authorize(&worker_key.public().to_string(), "mac/echo")
            .is_err()
    );
    mac.close().await;
    echo.abort();
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}

fn secret_hex(secret: &SecretKey) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in secret.to_bytes() {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 15)]));
    }
    result
}

async fn start_relay() -> Result<(iroh_relay::server::Server, String), Error> {
    let mut relay_config = iroh_relay::server::RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay_config.tls = None;
    let mut server_config = iroh_relay::server::ServerConfig::default();
    server_config.relay = Some(relay_config);
    server_config.metrics_addr = None;
    let relay = iroh_relay::server::Server::spawn(server_config)
        .await
        .map_err(transport)?;
    let relay_url = format!(
        "http://{}",
        relay
            .http_addr()
            .ok_or_else(|| Error::Transport("relay address missing".into()))?
    );
    Ok((relay, relay_url))
}

async fn start_echo() -> Result<(u16, tokio::task::JoinHandle<()>), Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let echo_port = listener.local_addr()?.port();
    let echo = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    Ok((echo_port, echo))
}

fn apply_termination(termination: Termination, topology: &Topology, unauthorized: &mut Topology) -> Result<(), Error> {
    match termination {
        Termination::Revoke => {}
        Termination::Expire => {
            unauthorized.grants = topology.grants.clone();
            unauthorized
                .grants
                .get_mut("worker-echo")
                .ok_or(Error::Denied)?
                .expires_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(transport)?
                .as_secs()
                + 1;
        }
        Termination::Rotate => {
            unauthorized.grants = topology.grants.clone();
            unauthorized.nodes.get_mut("worker").ok_or(Error::Denied)?.key =
                SecretKey::from_bytes(&[4; 32]).public().to_string();
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_status_clears_when_live_transport_is_lost() -> Result<(), Error> {
    let state = tempfile::tempdir()?;
    let (relay, url) = start_relay().await?;
    let key = SecretKey::from_bytes(&[1; 32]);
    let mut topology = Topology::empty("relay-status-island");
    topology.nodes.insert(
        "authority".into(),
        Node {
            key: key.public().to_string(),
        },
    );
    let (agent, _) = bind_agent(&key, "authority", &topology, &url, state.path()).await?;
    assert!(agent.relay_connected());
    relay.shutdown().await.map_err(transport)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while agent.relay_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| Error::Timeout)?;
    assert!(!agent.relay_connected());
    agent.shutdown().await;
    assert!(!agent.relay_connected());
    Ok(())
}
