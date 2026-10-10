use std::time::Duration;

use iroh::{RelayMode, endpoint::presets::Minimal};

use super::*;
use crate::{Grant, Node, SecretKey, Service, controller::unix_now};

#[cfg(windows)]
mod platform;
mod relay_policy;

#[tokio::test]
async fn untrusted_wire_requests_cannot_reach_undeclared_services() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), denied_scenario())
        .await
        .map_err(|_| Error::Timeout)?
}

async fn denied_scenario() -> Result<()> {
    let (relay, relay_url) = test_relay().await?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let worker = SecretKey::from_bytes(&[1; 32]);
    let mac = SecretKey::from_bytes(&[2; 32]);
    let other = SecretKey::from_bytes(&[3; 32]);
    let mut topology = Topology::empty("deny-island");
    for (name, key) in [("worker", &worker), ("mac", &mac), ("other", &other)] {
        topology.nodes.insert(
            name.into(),
            Node {
                key: key.public().to_string(),
            },
        );
    }
    for node in ["mac", "other"] {
        let id = format!("{node}/echo");
        topology.services.insert(
            id.clone(),
            Service {
                node: node.into(),
                port: listener.local_addr()?.port(),
            },
        );
        topology.grants.insert(
            id.clone(),
            Grant {
                from: vec!["worker".into()],
                to: id,
                expires_at: unix_now() + 60,
            },
        );
    }
    let controller = Controller::new(topology.clone())?;
    let config = AgentConfig {
        node: "mac".into(),
        secret_key: "02".repeat(32),
        authority_key: other.public().to_string(),
        topology: topology.clone(),
        relay_urls: vec![relay_url.clone()],
        relay_only: true,
    };
    let agent = Agent::bind(config, controller.clone()).await?;
    let endpoint = Endpoint::builder(Minimal)
        .secret_key(worker)
        .relay_mode(RelayMode::custom([relay_url.parse().map_err(transport)?]))
        .clear_ip_transports()
        .bind()
        .await
        .map_err(transport)?;
    agent.online().await?;
    endpoint.online().await;
    for (network, service) in [
        ("foreign-island", "mac/echo"),
        ("deny-island", "mac/undeclared"),
        ("deny-island", "other/echo"),
    ] {
        let request = Request::Connect {
            network: network.into(),
            service: service.into(),
        };
        assert!(!request_accepted(&endpoint, agent.addr(), request).await?);
    }
    topology.revision = 1;
    assert!(!request_accepted(&endpoint, agent.addr(), Request::Update { topology }).await?);
    let outsider = Endpoint::builder(Minimal)
        .relay_mode(RelayMode::custom([relay_url.parse().map_err(transport)?]))
        .clear_ip_transports()
        .bind()
        .await
        .map_err(transport)?;
    outsider.online().await;
    let request = Request::Connect {
        network: "deny-island".into(),
        service: "mac/echo".into(),
    };
    assert!(!request_accepted(&outsider, agent.addr(), request).await?);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(controller.status().sessions, 0);
    outsider.close().await;
    endpoint.close().await;
    agent.close().await;
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}

async fn request_accepted(endpoint: &Endpoint, address: EndpointAddr, request: Request) -> Result<bool> {
    let connection = endpoint.connect(address, wire::ALPN).await.map_err(transport)?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(transport)?;
    wire::write(&mut send, &request).await?;
    let response: Response = wire::read(&mut recv).await?;
    connection.close(0_u32.into(), b"test complete");
    Ok(response.accepted)
}

#[test]
fn no_relay_configuration_can_fall_back_to_public_servers() {
    assert!(validate_relays(&[]).is_err());
    assert!(validate_relays(&["http://relay.example.com".into()]).is_err());
    assert!(validate_relays(&["https://user:secret@relay.example.com".into()]).is_err());
    assert!(validate_relays(&["https://relay.example.com?token=secret".into()]).is_err());
    assert_eq!(
        validate_relays(&["https://relay.example.com".into()])
            .map(|relays| relays.len())
            .ok(),
        Some(1)
    );
}

#[tokio::test]
async fn response_completion_cannot_hold_connection_slots_forever() {
    let completion = std::future::pending::<()>();
    assert!(matches!(
        bounded_response(completion, Duration::from_millis(10)).await,
        Err(Error::Timeout)
    ));
}

#[tokio::test]
#[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
async fn withdrawn_persistent_identity_starts_denied_and_reenrolls_only_by_new_authority_update() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), withdrawn_restart())
        .await
        .map_err(|_| Error::Timeout)?
}
async fn withdrawn_restart() -> Result<()> {
    let (relay, relay_url) = test_relay().await?;
    let directory = tempfile::tempdir()?;
    let (config, initial, authority_key) = withdrawal_fixture(&relay_url);
    let agent = Agent::bind_with_store(config.clone(), directory.path().join("worker")).await?;
    let authority = Endpoint::builder(Minimal)
        .secret_key(authority_key)
        .relay_mode(RelayMode::custom([relay_url.parse().map_err(transport)?]))
        .clear_ip_transports()
        .bind()
        .await
        .map_err(transport)?;
    agent.online().await?;
    authority.online().await;
    let mut removed = initial.clone();
    removed.revision = 1;
    removed.nodes.remove("worker");
    assert!(
        request_accepted(
            &authority,
            agent.addr(),
            Request::Update {
                topology: removed.clone()
            }
        )
        .await?
    );
    agent.shutdown().await;
    drop(agent);
    let restarted = Agent::bind_with_store(config.clone(), directory.path().join("worker")).await?;
    restarted.online().await?;
    assert_eq!(restarted.controller().topology(), removed);
    assert!(
        !request_accepted(
            &authority,
            restarted.addr(),
            Request::Probe {
                network: removed.network.clone()
            }
        )
        .await?
    );
    assert_unenrolled_rejected(&config, &removed).await?;
    let mut restored = initial;
    restored.revision = 2;
    assert!(
        request_accepted(
            &authority,
            restarted.addr(),
            Request::Update {
                topology: restored.clone()
            }
        )
        .await?
    );
    assert!(
        request_accepted(
            &authority,
            restarted.addr(),
            Request::Probe {
                network: restored.network.clone()
            }
        )
        .await?
    );
    let mut rotated = restored;
    rotated.revision = 3;
    rotated.nodes.insert(
        "worker".into(),
        Node {
            key: SecretKey::from_bytes(&[23; 32]).public().to_string(),
        },
    );
    assert!(request_accepted(&authority, restarted.addr(), Request::Update { topology: rotated }).await?);
    restarted.shutdown().await;
    drop(restarted);
    assert!(
        Agent::bind_with_store(config, directory.path().join("worker"))
            .await
            .is_err()
    );
    authority.close().await;
    relay.shutdown().await.map_err(transport)?;
    Ok(())
}

async fn assert_unenrolled_rejected(config: &AgentConfig, removed: &Topology) -> Result<()> {
    assert!(
        Agent::bind(
            AgentConfig {
                topology: removed.clone(),
                ..config.clone()
            },
            Controller::new(removed.clone())?
        )
        .await
        .is_err()
    );
    Ok(())
}

async fn test_relay() -> Result<(iroh_relay::server::Server, String)> {
    let mut relay = iroh_relay::server::RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = None;
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    config.metrics_addr = None;
    let server = iroh_relay::server::Server::spawn(config).await.map_err(transport)?;
    let url = format!("http://{}", server.http_addr().ok_or(Error::Denied)?);
    Ok((server, url))
}

fn withdrawal_fixture(relay_url: &str) -> (AgentConfig, Topology, SecretKey) {
    let authority_key = SecretKey::from_bytes(&[21; 32]);
    let worker = SecretKey::from_bytes(&[22; 32]);
    let mut initial = Topology::empty("withdrawn-island");
    initial.nodes.insert(
        "authority".into(),
        Node {
            key: authority_key.public().to_string(),
        },
    );
    initial.nodes.insert(
        "worker".into(),
        Node {
            key: worker.public().to_string(),
        },
    );
    let config = AgentConfig {
        node: "worker".into(),
        secret_key: "16".repeat(32),
        authority_key: authority_key.public().to_string(),
        topology: initial.clone(),
        relay_urls: vec![relay_url.into()],
        relay_only: true,
    };
    (config, initial, authority_key)
}
