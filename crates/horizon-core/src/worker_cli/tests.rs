use super::{Error, storage};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn durable_claim_is_never_overwritten_after_an_uncertain_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let claim = root.path().join("start.claimed");
    storage::write_new(&claim, b"first").unwrap();
    assert!(matches!(storage::write_new(&claim, b"replayed"), Err(Error::Storage)));
    assert_eq!(fs::read(&claim).unwrap(), b"first");
    assert_eq!(fs::metadata(&claim).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn competing_controller_cannot_acquire_task_lock() {
    let root = tempfile::tempdir().unwrap();
    let first = storage::lock(root.path()).unwrap();
    assert!(matches!(storage::lock(root.path()), Err(Error::Storage)));
    drop(first);
    assert!(storage::lock(root.path()).is_ok());
}

#[test]
fn task_creation_refuses_existing_directory_without_changing_it() {
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("keep");
    fs::write(&sentinel, b"existing").unwrap();
    assert!(storage::create_root(root.path()).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"existing");
}

#[test]
fn task_storage_requires_private_directory_permissions() {
    let parent = tempfile::tempdir().unwrap();
    let root = storage::create_root(&parent.path().join("task")).unwrap();
    assert!(storage::private_directory(&root).is_ok());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(storage::private_directory(&root).is_err());
}

#[test]
fn missing_setup_receipt_is_observed_without_creating_workspace_or_replaying() {
    use horizon_core::{HorizonHome, RuntimeState, SessionStore};
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    storage::create_root(&root.path().join("home")).unwrap();
    let home = HorizonHome::from_root(root.path().join("home"));
    let session = SessionStore::new(home, root.path().join("profile.json"))
        .create_session_from_runtime(RuntimeState::default())
        .unwrap();
    let intent = serde_json::from_value(json!({
        "config": {"local_docker": [{"name": "fixture", "docker_host": "unix:///nonexistent/docker.sock"}]},
        "target": {"provider": "local_docker", "profile": "fixture",
            "image": format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "disk_gib": 20, "lifetime": "persistent"},
        "repository": {"repository": "fixture/repository", "commit": "a".repeat(40), "branch": "test/fixture"},
        "command": {"program": "/bin/false", "args": []}, "working_directory": ".",
        "setup_expires_at_millis": 1, "issue": "fixture"
    }))
    .unwrap();
    let receipt = storage::Receipt {
        version: 1,
        root: root.path().to_owned(),
        session: session.session_id,
        workspace: "missing-workspace".into(),
        panel: "missing-panel".into(),
        intent,
    };
    storage::write_new(
        &root.path().join("receipt.json"),
        &serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let context = storage::Context::new(root.path(), receipt, storage::lock(root.path()).unwrap());
    context.claim("create").unwrap();
    // A missing/corrupt database remains an explicit storage error, never repaired.
    assert!(super::operations::check(&context).is_err());
    horizon_core::cloud_run::CloudWorkflowStore::open(&context.home).unwrap();
    for _ in 0..2 {
        assert_eq!(super::operations::check(&context).unwrap()["setup"], "missing");
        assert!(context.saved().is_err());
    }
    assert!(matches!(context.claim("create"), Err(Error::Claimed)));
}
