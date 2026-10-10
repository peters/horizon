use super::*;
use crate::{AgentConfig, store::Store};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn config(with_grant: bool) -> AgentConfig {
    let mut topology = Topology::empty("controller-store-island");
    for (name, byte) in [("source", 1), ("destination", 2)] {
        topology.nodes.insert(
            name.into(),
            Node {
                key: SecretKey::from_bytes(&[byte; 32]).public().to_string(),
            },
        );
    }
    topology.services.insert(
        "ssh".into(),
        Service {
            node: "destination".into(),
            port: 22,
        },
    );
    if with_grant {
        topology.grants.insert(
            "lease".into(),
            crate::Grant {
                from: vec!["source".into()],
                to: "ssh".into(),
                expires_at: unix_now() + 3600,
            },
        );
    }
    AgentConfig {
        node: "destination".into(),
        secret_key: String::new(),
        authority_key: topology.nodes["source"].key.clone(),
        topology,
        relay_urls: vec!["https://relay.example.com".into()],
        relay_only: true,
    }
}

#[test]
fn public_apply_and_revoke_survive_restart_and_clones_retain_writer_ownership() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().join("state");
    let mut initial = config(false);
    let store = Store::open(directory.clone(), &mut initial)?;
    let controller = Controller::new(initial.topology.clone())?;
    controller.bind_store(store)?;
    let clone = controller.clone();
    let mut proposed = config(true).topology;
    proposed.revision = 1;
    let plan = clone.plan(proposed.clone())?;
    assert!(clone.apply(&plan)?.changed);
    assert!(!controller.apply(&plan)?.changed);
    drop(controller);
    assert!(Store::open(directory.clone(), &mut config(false)).is_err());
    drop(clone);
    let mut enrollment = config(false);
    let store = Store::open(directory.clone(), &mut enrollment)?;
    assert_eq!(enrollment.topology, proposed);
    let controller = Controller::new(enrollment.topology.clone())?;
    controller.bind_store(store)?;
    assert!(controller.revoke("lease")?);
    assert!(!controller.revoke("lease")?);
    assert_eq!(controller.topology().revision, 2);
    drop(controller);
    let mut enrollment = config(true);
    let _store = Store::open(directory, &mut enrollment)?;
    assert_eq!(enrollment.topology.revision, 2);
    assert!(enrollment.topology.grants.is_empty());
    Ok(())
}

#[tokio::test]
async fn failed_public_apply_and_revoke_preserve_policy_and_live_session() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().join("state");
    let mut initial = config(true);
    let store = Store::open(directory.clone(), &mut initial)?;
    let controller = Controller::new(initial.topology.clone())?;
    controller.bind_store(store)?;
    let source = Endpoint::builder(Minimal)
        .secret_key(SecretKey::from_bytes(&[1; 32]))
        .bind_addr("127.0.0.1:0")
        .map_err(|error| Error::Transport(error.to_string()))?
        .bind()
        .await
        .map_err(|error| Error::Transport(error.to_string()))?;
    let destination = Endpoint::builder(Minimal)
        .secret_key(SecretKey::from_bytes(&[2; 32]))
        .bind_addr("127.0.0.1:0")
        .map_err(|error| Error::Transport(error.to_string()))?
        .alpns(vec![b"persistence-test".to_vec()])
        .bind()
        .await
        .map_err(|error| Error::Transport(error.to_string()))?;
    let accepting = destination.clone();
    let accepted = tokio::spawn(async move {
        accepting
            .accept()
            .await
            .ok_or(Error::Denied)?
            .await
            .map_err(|error| Error::Transport(error.to_string()))
    });
    let connection = tokio::time::timeout(
        Duration::from_secs(5),
        source.connect(destination.addr(), b"persistence-test"),
    )
    .await
    .map_err(|_| Error::Timeout)?
    .map_err(|error| Error::Transport(error.to_string()))?;
    let server_connection = accepted.await.map_err(|error| Error::Transport(error.to_string()))??;
    let id = controller.register(source.id(), destination.id(), "ssh", 22, connection.clone())?;
    controller.confirmed(id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut peer = tokio::net::TcpStream::connect(listener.local_addr()?).await?;
    let (socket, _) = listener.accept().await?;
    let mut socket = controller.attach_socket(id, socket)?;
    let blocked = directory.join("00000000000000000001.json");
    std::fs::create_dir(&blocked)?;
    let mut proposed = initial.topology.clone();
    proposed.revision = 1;
    proposed.grants.clear();
    assert!(controller.apply(&controller.plan(proposed)?).is_err());
    assert!(controller.revoke("lease").is_err());
    assert_eq!(controller.topology(), initial.topology);
    assert_eq!(controller.status().sessions, 1);
    assert!(connection.close_reason().is_none());
    peer.write_all(b"live").await?;
    let mut bytes = [0; 4];
    tokio::time::timeout(Duration::from_secs(3), socket.read_exact(&mut bytes))
        .await
        .map_err(|_| Error::Timeout)??;
    assert_eq!(&bytes, b"live");
    std::fs::remove_dir(&blocked)?;
    controller.unregister(id);
    drop(controller);
    let mut enrollment = config(false);
    let _store = Store::open(directory, &mut enrollment)?;
    assert_eq!(enrollment.topology, initial.topology);
    connection.close(0_u32.into(), b"test complete");
    server_connection.close(0_u32.into(), b"test complete");
    source.close().await;
    destination.close().await;
    Ok(())
}
