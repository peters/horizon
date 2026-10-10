use std::time::Duration;

use iroh::{Endpoint, SecretKey, endpoint::presets::Minimal};

use super::*;
use crate::{Grant, Node, Service};

#[tokio::test]
async fn changed_target_between_authorization_and_registration_is_denied() -> Result<()> {
    let source_key = SecretKey::from_bytes(&[1; 32]);
    let destination_key = SecretKey::from_bytes(&[2; 32]);
    let source = Endpoint::builder(Minimal)
        .secret_key(source_key.clone())
        .bind_addr("127.0.0.1:0")
        .map_err(|error| Error::Transport(error.to_string()))?
        .bind()
        .await
        .map_err(|error| Error::Transport(error.to_string()))?;
    let destination = Endpoint::builder(Minimal)
        .secret_key(destination_key.clone())
        .bind_addr("127.0.0.1:0")
        .map_err(|error| Error::Transport(error.to_string()))?
        .alpns(vec![b"test".to_vec()])
        .bind()
        .await
        .map_err(|error| Error::Transport(error.to_string()))?;
    let accepting = destination.clone();
    let accept = tokio::spawn(async move {
        let incoming = accepting.accept().await.ok_or(Error::Denied)?;
        incoming.await.map_err(|error| Error::Transport(error.to_string()))
    });
    let connection = tokio::time::timeout(Duration::from_secs(5), source.connect(destination.addr(), b"test"))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|error| Error::Transport(error.to_string()))?;
    let server_connection = accept.await.map_err(|error| Error::Transport(error.to_string()))??;
    let mut topology = Topology::empty("race-island");
    topology.nodes.insert(
        "source".into(),
        Node {
            key: source_key.public().to_string(),
        },
    );
    topology.nodes.insert(
        "destination".into(),
        Node {
            key: destination_key.public().to_string(),
        },
    );
    topology.services.insert(
        "ssh".into(),
        Service {
            node: "destination".into(),
            port: 22,
        },
    );
    topology.grants.insert(
        "lease".into(),
        Grant {
            from: vec!["source".into()],
            to: "ssh".into(),
            expires_at: unix_now() + 60,
        },
    );
    let controller = Controller::new(topology.clone())?;
    let previous = controller.authorize(&source_key.public().to_string(), "ssh")?;
    topology.revision = 1;
    topology.services.get_mut("ssh").ok_or(Error::Denied)?.port = 23;
    controller.apply(&controller.plan(topology)?)?;
    assert!(matches!(
        controller.register(
            source_key.public(),
            destination_key.public(),
            "ssh",
            previous.port,
            connection
        ),
        Err(Error::Denied)
    ));
    assert_eq!(controller.status().sessions, 0);
    server_connection.close(0_u32.into(), b"test complete");
    source.close().await;
    destination.close().await;
    Ok(())
}
