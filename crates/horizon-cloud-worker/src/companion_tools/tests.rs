use super::*;
use horizon_cloud_protocol::companion::VERSION;

fn catalog(status: Status) -> Catalog {
    Catalog {
        version: VERSION,
        source_cloud_id: "cloud-source".into(),
        observed_at: 100,
        companions: vec![Companion {
            alias: "app".into(),
            repository: "example/app".into(),
            profile: "cpu".into(),
            target_cloud_id: Some("cloud-app".into()),
            selected: true,
            status,
            access: (status == Status::Ready).then(|| Access {
                grant: "grant-app".into(),
                ssh_alias: "companion-app".into(),
                worktree: "/workspace/companions/worktrees/grant-app".into(),
            }),
        }],
    }
}

fn save(path: &Path, catalog: &Catalog) {
    std::fs::write(path, serde_json::to_vec(catalog).unwrap()).unwrap();
}

#[test]
fn stale_and_future_snapshots_do_not_claim_ready_and_never_mutate_catalog() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let original = catalog(Status::Ready);
    save(file.path(), &original);
    assert_eq!(list(file.path(), 160).unwrap().companions[0].status, Status::Ready);
    for time in [99, 161, u64::MAX] {
        assert_eq!(
            list(file.path(), time).unwrap().companions[0].status,
            Status::Unverified
        );
    }
    assert_eq!(load(file.path()).unwrap(), original);
}

#[test]
fn inspection_probes_only_existing_access_and_preserves_stopped_state() {
    let file = tempfile::NamedTempFile::new().unwrap();
    save(file.path(), &catalog(Status::Stopped));
    let entry = inspect(file.path(), "app", 100, |_| {
        panic!("stopped target must not be contacted")
    })
    .unwrap();
    assert_eq!(entry.status, Status::Stopped);
    save(file.path(), &catalog(Status::Ready));
    let entry = inspect(file.path(), "app", 200, |_| Ok(true)).unwrap();
    assert_eq!(entry.status, Status::Ready);
    let entry = inspect(file.path(), "app", 200, |_| Err(io::Error::other("network offline"))).unwrap();
    assert_eq!(entry.status, Status::Unreachable);
    assert!(
        inspect(file.path(), "unknown", 100, |_| panic!(
            "unknown alias must not be probed"
        ))
        .is_err()
    );
}

#[test]
fn changed_selection_during_probe_does_not_publish_old_readiness() {
    let file = tempfile::NamedTempFile::new().unwrap();
    save(file.path(), &catalog(Status::Ready));
    let result = inspect(file.path(), "app", 100, |_| {
        let mut changed = catalog(Status::RevocationPending);
        changed.companions[0].selected = false;
        save(file.path(), &changed);
        Ok(true)
    });
    assert!(result.is_err());
}

#[test]
fn changed_source_during_probe_does_not_publish_old_readiness() {
    let file = tempfile::NamedTempFile::new().unwrap();
    save(file.path(), &catalog(Status::Ready));
    let result = inspect(file.path(), "app", 100, |_| {
        let mut changed = catalog(Status::Ready);
        changed.source_cloud_id = "another-cloud".into();
        save(file.path(), &changed);
        Ok(true)
    });
    assert!(result.is_err());
}

#[test]
fn malformed_and_unselected_connections_are_rejected_before_any_probe() {
    let original = catalog(Status::Ready);
    let mut invalid = original.clone();
    invalid.companions[0].alias = "x;touch /tmp/injected".into();
    assert!(invalid.validate().is_err());
    invalid = original.clone();
    invalid.companions[0].selected = false;
    assert!(invalid.validate().is_err());
    invalid = original.clone();
    invalid.companions[0].access.as_mut().unwrap().worktree = "/workspace/agents/active".into();
    assert!(invalid.validate().is_err());
    invalid = original.clone();
    invalid.companions.push(original.companions[0].clone());
    assert!(invalid.validate().is_err());
    invalid = original;
    invalid.companions[0].access = None;
    assert!(invalid.validate().is_err());
    assert!(
        decode(&b"private invalid data"[..])
            .unwrap_err()
            .to_string()
            .contains("Invalid companion catalog")
    );
    assert!(decode(vec![b' '; usize::try_from(MAX_CATALOG_BYTES).unwrap() + 1].as_slice()).is_err());
}

#[test]
fn aliases_cannot_share_a_connection_grant() {
    let mut snapshot = catalog(Status::Ready);
    let mut other = snapshot.companions[0].clone();
    other.alias = "utility".into();
    other.target_cloud_id = Some("cloud-utility".into());
    other.access.as_mut().unwrap().ssh_alias = "companion-utility".into();
    snapshot.companions.push(other);
    assert!(snapshot.validate().is_err());

    let access = snapshot.companions[1].access.as_mut().unwrap();
    access.grant = "grant-utility".into();
    access.worktree = "/workspace/companions/worktrees/grant-utility".into();
    assert!(snapshot.validate().is_ok());
}

#[test]
fn missing_catalog_is_an_explicit_discovery_error() {
    let root = tempfile::tempdir().unwrap();
    assert!(list(&root.path().join("missing"), 100).is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
