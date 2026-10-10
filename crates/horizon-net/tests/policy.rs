use horizon_net::{Controller, Error, Grant, Node, SecretKey, Service, Topology};

fn topology() -> Topology {
    let mut topology = Topology::empty("test-island");
    topology.nodes.insert(
        "worker".into(),
        Node {
            key: SecretKey::from_bytes(&[1; 32]).public().to_string(),
        },
    );
    topology.nodes.insert(
        "mac".into(),
        Node {
            key: SecretKey::from_bytes(&[2; 32]).public().to_string(),
        },
    );
    topology.services.insert(
        "mac/ssh".into(),
        Service {
            node: "mac".into(),
            port: 22,
        },
    );
    topology
}

#[test]
fn plans_are_pure_exact_and_idempotent() -> Result<(), Error> {
    let initial = topology();
    let controller = Controller::new(initial.clone())?;
    let mut next = initial.clone();
    next.revision = 1;
    next.grants.insert(
        "session".into(),
        Grant {
            from: vec!["worker".into()],
            to: "mac/ssh".into(),
            expires_at: u64::MAX,
        },
    );
    let plan = controller.plan(next.clone())?;
    assert_eq!(controller.topology(), initial);
    assert!(plan.widens_access);
    assert_eq!(plan.changes.len(), 1);
    let mut tampered = plan.clone();
    tampered.widens_access = false;
    assert!(matches!(controller.apply(&tampered), Err(Error::StalePlan)));
    assert!(controller.apply(&plan)?.changed);
    assert!(!controller.apply(&plan)?.changed);
    assert_eq!(controller.topology(), next);
    assert!(controller.revoke("session")?);
    assert!(!controller.revoke("session")?);
    assert!(matches!(controller.apply(&plan), Err(Error::StalePlan)));
    Ok(())
}

#[test]
fn default_deny_expiry_and_identity_are_enforced() -> Result<(), Error> {
    let mut topology = topology();
    let source = topology.nodes["worker"].key.clone();
    let controller = Controller::new(topology.clone())?;
    assert!(matches!(controller.authorize(&source, "mac/ssh"), Err(Error::Denied)));
    topology.revision = 1;
    topology.grants.insert(
        "expired".into(),
        Grant {
            from: vec!["worker".into()],
            to: "mac/ssh".into(),
            expires_at: 1,
        },
    );
    controller.apply(&controller.plan(topology.clone())?)?;
    assert!(matches!(controller.authorize(&source, "mac/ssh"), Err(Error::Denied)));
    topology.revision = 2;
    topology.grants.get_mut("expired").ok_or(Error::Denied)?.expires_at = u64::MAX;
    controller.apply(&controller.plan(topology)?)?;
    assert_eq!(controller.authorize(&source, "mac/ssh")?.port, 22);
    let outsider = SecretKey::from_bytes(&[3; 32]).public().to_string();
    assert!(matches!(controller.authorize(&outsider, "mac/ssh"), Err(Error::Denied)));
    assert!(matches!(controller.authorize(&source, "mac/vnc"), Err(Error::Denied)));
    Ok(())
}

#[test]
fn cross_network_replays_and_duplicate_keys_fail() -> Result<(), Error> {
    let initial = topology();
    let controller = Controller::new(initial.clone())?;
    let mut foreign = initial.clone();
    foreign.network = "other-island".into();
    foreign.revision = 1;
    assert!(controller.plan(foreign).is_err());
    let mut duplicate = initial;
    duplicate
        .nodes
        .insert("impostor".into(), duplicate.nodes["worker"].clone());
    assert!(Controller::new(duplicate).is_err());
    Ok(())
}
