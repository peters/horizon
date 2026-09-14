use super::{Error, storage};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn invalid_command_arguments_never_reach_storage_or_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("never-created");
    let path = missing.to_str().unwrap();
    for arguments in [
        vec![],
        vec!["create"],
        vec!["unknown", path],
        vec!["create", path, "panel"],
        vec!["manifest", path, "panel"],
        vec!["delete", path, "panel"],
        vec!["check", path, "panel"],
        vec!["status", path, "panel", "extra"],
    ] {
        let result = super::run_with_args(arguments.into_iter().map(Into::into));
        assert!(matches!(result, Err(Error::Usage)));
        assert!(!missing.exists());
    }
    for operation in ["start", "status", "snapshot"] {
        let result = super::run_with_args([operation, path, "panel"].into_iter().map(Into::into));
        assert!(matches!(result, Err(Error::Storage)));
        assert!(!missing.exists());
    }
}

#[test]
fn failed_creation_retains_identity_and_prevents_a_second_creation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("task");
    let socket = directory.path().join("missing.sock");
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 300_000;
    let request = serde_json::json!({
        "config": {"local_docker": [{"name": "fixture", "docker_host": format!("unix://{}", socket.display())}]},
        "target": {"provider": "local_docker", "profile": "fixture",
            "image": format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "disk_gib": 20, "lifetime": "persistent"},
        "repository": {"repository": "fixture/repository", "commit": "a".repeat(40), "branch": "test/fixture"},
        "command": {"program": "/bin/false", "args": []}, "working_directory": ".",
        "setup_expires_at_millis": expiry, "issue": "fixture"
    });
    let intent = || serde_json::from_value(request.clone()).unwrap();
    assert!(matches!(
        super::operations::create(&root, intent()),
        Err(Error::Remote(_))
    ));
    let receipt = fs::read(root.join("receipt.json")).unwrap();
    let claim = fs::read(root.join("create.claimed")).unwrap();
    let context = storage::Context::open(&root).unwrap();
    let identity = (context.receipt.session.clone(), context.receipt.workspace.clone());
    // The original failed allocation is inspectable; observations never retry create.
    let _ = super::operations::check(&context);
    assert_eq!(fs::read(root.join("receipt.json")).unwrap(), receipt);
    assert_eq!(fs::read(root.join("create.claimed")).unwrap(), claim);
    drop(context);
    assert!(matches!(
        super::operations::create(&root, intent()),
        Err(Error::Storage)
    ));
    assert_eq!(fs::read(root.join("receipt.json")).unwrap(), receipt);
    let reopened = storage::Context::open(&root).unwrap();
    assert_eq!(
        (reopened.receipt.session.clone(), reopened.receipt.workspace.clone()),
        identity
    );
    assert!(!root.join("git.claimed").exists());
    assert!(!socket.exists());
}

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

#[test]
fn incomplete_journal_publication_does_not_strand_or_overwrite_intent() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("add-operation.json");
    let mut interrupted = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    interrupted.write_all(b"partial").unwrap();
    assert!(!path.exists());
    storage::publish_journal(&path, b"complete").unwrap();
    assert!(storage::publish_journal(&path, b"replacement").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    drop(interrupted);
    assert_eq!(fs::read(path).unwrap(), b"complete");
}
