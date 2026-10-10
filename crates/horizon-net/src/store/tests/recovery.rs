use super::*;

#[test]
#[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
fn invalid_enrollment_identity_cannot_create_or_poison_state() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    for invalid_kind in 0..6 {
        let directory = temporary.path().join(format!("invalid-{invalid_kind}"));
        let mut invalid = config();
        match invalid_kind {
            0 => invalid.authority_key = "malformed authority".into(),
            1 => invalid.secret_key = "malformed secret".into(),
            2 => invalid.secret_key = "02".repeat(32),
            3 => invalid.topology.nodes.get_mut("node").ok_or(Error::Denied)?.key = "malformed node key".into(),
            4 => {
                invalid.topology.nodes.clear();
            }
            _ => invalid.relay_urls.clear(),
        }
        assert!(Store::open(directory.clone(), &mut invalid).is_err());
        assert!(
            !directory.exists(),
            "invalid identity must fail before directory or writer creation"
        );
        let store = Store::open(directory.clone(), &mut config())?;
        assert!(!store.awaiting_confirmation());
        drop(store);
        let original = fs::read(directory.join("00000000000000000000.json"))?;
        assert!(Store::open(directory.clone(), &mut invalid).is_err());
        assert_eq!(fs::read(directory.join("00000000000000000000.json"))?, original);
        let reopened = Store::open(directory, &mut config())?;
        assert!(reopened.awaiting_confirmation());
    }
    Ok(())
}

#[test]
#[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
fn acknowledged_persistent_grant_also_requires_confirmation_after_every_restart() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().join("state");
    let mut enrolled = granted_config();
    let store = Store::open(directory.clone(), &mut enrolled)?;
    let controller = crate::Controller::new(enrolled.topology.clone())?;
    controller.bind_store(store)?;
    let source = enrolled.topology.nodes["node"].key.clone();
    assert_eq!(controller.status().policy_state, crate::PolicyState::Confirmed);
    assert!(controller.authorize(&source, "self-service").is_ok());
    drop(controller);
    for _ in 0..2 {
        let store = Store::open(directory.clone(), &mut enrolled)?;
        let controller = crate::Controller::new(enrolled.topology.clone())?;
        controller.bind_store(store)?;
        assert_eq!(
            controller.status().policy_state,
            crate::PolicyState::AwaitingConfirmation
        );
        assert!(controller.status().grants.iter().all(|grant| !grant.active));
        assert!(matches!(
            controller.authorize(&source, "self-service"),
            Err(Error::Denied)
        ));
        assert!(!controller.apply(&controller.plan(enrolled.topology.clone())?)?.changed);
        assert_eq!(controller.status().policy_state, crate::PolicyState::Confirmed);
        assert!(controller.authorize(&source, "self-service").is_ok());
        drop(controller);
    }
    Ok(())
}

fn granted_config() -> AgentConfig {
    let mut initial = config();
    initial.topology.services.insert(
        "self-service".into(),
        crate::Service {
            node: "node".into(),
            port: 22,
        },
    );
    initial.topology.grants.insert(
        "lease".into(),
        crate::Grant {
            from: vec!["node".into()],
            to: "self-service".into(),
            expires_at: u64::MAX,
        },
    );
    initial
}

// This test needs actual Unix directory modes and a non-root owner for a real failed fsync barrier.
#[cfg(unix)]
#[test]
fn failed_widening_barrier_restart_denies_until_exact_confirmation_is_durable() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return Ok(());
    }
    let temporary = tempfile::tempdir()?;
    let ancestor = temporary.path().join("owned-ancestor");
    fs::create_dir(&ancestor)?;
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))?;
    let directory = ancestor.join("nested/state");
    let mut initial = granted_config();
    initial.topology.grants.clear();
    let store = Store::open(directory.clone(), &mut initial)?;
    let controller = crate::Controller::new(initial.topology.clone())?;
    controller.bind_store(store)?;
    let mut widened = granted_config().topology;
    widened.revision = 1;
    let source = widened.nodes["node"].key.clone();
    let restore = RestoreAncestorPermissions(ancestor.clone());
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o300))?;
    assert!(
        matches!(controller.apply(&controller.plan(widened.clone())?), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
    );
    assert_eq!(controller.topology(), initial.topology);
    assert!(directory.join("00000000000000000001.json").is_file());
    assert!(fs::File::open(&directory)?.sync_all().is_ok());
    assert!(fs::File::open(&ancestor).is_err());
    drop(controller);
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))?;
    drop(restore);
    let store = Store::open(directory.clone(), &mut initial)?;
    assert_eq!(
        initial.topology, widened,
        "retain uncertain revision for exact authority confirmation"
    );
    let controller = crate::Controller::new(initial.topology.clone())?;
    controller.bind_store(store)?;
    assert_eq!(
        controller.status().policy_state,
        crate::PolicyState::AwaitingConfirmation
    );
    assert!(matches!(
        controller.authorize(&source, "self-service"),
        Err(Error::Denied)
    ));
    let exact = controller.plan(widened.clone())?;
    let restore = RestoreAncestorPermissions(ancestor.clone());
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o300))?;
    for _ in 0..2 {
        assert!(
            matches!(controller.apply(&exact), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert_eq!(
            controller.status().policy_state,
            crate::PolicyState::AwaitingConfirmation
        );
        assert!(matches!(
            controller.authorize(&source, "self-service"),
            Err(Error::Denied)
        ));
    }
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))?;
    drop(restore);
    assert!(!controller.apply(&exact)?.changed);
    assert_eq!(controller.status().policy_state, crate::PolicyState::Confirmed);
    assert_eq!(controller.topology(), widened);
    assert!(controller.authorize(&source, "self-service").is_ok());
    assert!(!controller.apply(&exact)?.changed);
    assert_eq!(
        fs::read_dir(&directory)?.count(),
        3,
        "original revision, uncertain revision, writer lock; no fabricated marker"
    );
    Ok(())
}

#[test]
#[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
fn equivalent_identity_spellings_preserve_snapshot_and_restart_quarantine() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().join("state");
    let original = granted_config();
    let store = Store::open(directory.clone(), &mut original.clone())?;
    let identity = store.enrolled_key()?;
    drop(store);
    let path = directory.join("00000000000000000000.json");
    let snapshot = fs::read(&path)?;
    let source = original.topology.nodes["node"].key.clone();
    for (authority_alias, node_alias) in [(true, false), (false, true), (true, true)] {
        let mut enrolled = original.clone();
        if authority_alias {
            enrolled.authority_key = format!("ed25519:{}", enrolled.authority_key);
        }
        if node_alias {
            enrolled.topology.nodes.get_mut("node").ok_or(Error::Denied)?.key = format!("ed25519:{source}");
        }
        let store = Store::open(directory.clone(), &mut enrolled)?;
        assert_eq!(store.enrolled_key()?, identity);
        assert_eq!(enrolled.topology, original.topology);
        assert_eq!(fs::read(&path)?, snapshot);
        let controller = crate::Controller::new(enrolled.topology.clone())?;
        controller.bind_store(store)?;
        assert_eq!(
            controller.status().policy_state,
            crate::PolicyState::AwaitingConfirmation
        );
        assert!(controller.status().grants.iter().all(|grant| !grant.active));
        assert!(matches!(
            controller.authorize(&source, "self-service"),
            Err(Error::Denied)
        ));
        assert!(!controller.apply(&controller.plan(original.topology.clone())?)?.changed);
        assert_eq!(controller.status().policy_state, crate::PolicyState::Confirmed);
        assert!(controller.authorize(&source, "self-service").is_ok());
        assert_eq!(controller.topology(), original.topology);
        assert_eq!(fs::read(&path)?, snapshot);
        assert_eq!(fs::read_dir(&directory)?.count(), 2);
        drop(controller);
    }
    let other_key = iroh::SecretKey::from_bytes(&[2; 32]).public().to_string();
    let mut different_authority = original.clone();
    different_authority.authority_key = format!("ed25519:{other_key}");
    assert!(matches!(
        Store::open(directory.clone(), &mut different_authority),
        Err(Error::InvalidConfiguration(_))
    ));
    let mut different_node = original.clone();
    different_node.secret_key = "02".repeat(32);
    different_node.topology.nodes.get_mut("node").ok_or(Error::Denied)?.key = format!("ed25519:{other_key}");
    assert!(matches!(
        Store::open(directory.clone(), &mut different_node),
        Err(Error::InvalidConfiguration(_))
    ));
    assert_eq!(fs::read(&path)?, snapshot);
    assert_eq!(fs::read_dir(&directory)?.count(), 2);
    Ok(())
}
