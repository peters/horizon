use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn configuration(root: &Path) -> serde_json::Value {
    serde_json::json!({"version":1,"owner":Uuid::new_v4(),"project":root,"state":root,
        "provider":"browserstack","tunnel_binary":"/synthetic/BrowserStackLocal","tunnel_sha256":"a".repeat(64)})
}
#[test]
fn private_client_refuses_shared_files_symlinks_fifos_oversize_and_extra_fields() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let path = root.join("client.json");
    let valid = configuration(&root);
    std::fs::write(&path, valid.to_string()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    client(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(client(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&path, root.join("alias.json")).unwrap();
    assert!(client(&root.join("alias.json")).is_err());
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .args(["-m", "600"])
            .arg(root.join("pipe.json"))
            .status()
            .unwrap()
            .success()
    );
    assert!(client(&root.join("pipe.json")).is_err());
    for invalid in [
        serde_json::json!({"version":1}),
        {
            let mut extra = valid.clone();
            extra["credential"] = "forbidden".into();
            extra
        },
        {
            let mut nil = valid.clone();
            nil["owner"] = Uuid::nil().to_string().into();
            nil
        },
    ] {
        std::fs::write(&path, invalid.to_string()).unwrap();
        assert!(client(&path).is_err());
    }
    std::fs::write(&path, vec![b'x'; 65537]).unwrap();
    assert!(client(&path).is_err());
}
#[test]
fn child_selection_preserves_existing_private_state_and_refuses_redirects() {
    use horizon_app_process::storage::Directory;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let held = Directory::open(&root).unwrap();
    child(&held, "00000000000000000000000000000001").unwrap();
    std::fs::write(root.join("00000000000000000000000000000001/retained"), "pending").unwrap();
    child(&held, "00000000000000000000000000000001").unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("00000000000000000000000000000001/retained")).unwrap(),
        "pending"
    );
    symlink(
        root.join("00000000000000000000000000000001"),
        root.join("00000000000000000000000000000003"),
    )
    .unwrap();
    assert!(child(&held, "00000000000000000000000000000003").is_err());
}

#[test]
fn restarted_launcher_refuses_untracked_crash_exports_without_deleting_them() {
    use horizon_app_process::storage::Directory;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let held = Directory::open(&root).unwrap();
    let name = "00000000000000000000000000000002";
    evidence_ready(&held, name).unwrap();
    let export = root.join(name).join("stale-frame.png");
    std::fs::write(&export, b"private retained capture").unwrap();
    assert_eq!(evidence_ready(&held, name), Err(Error::CleanupUncertain));
    assert_eq!(std::fs::read(&export).unwrap(), b"private retained capture");
    assert_eq!(std::fs::read_dir(root.join(name)).unwrap().count(), 1);
}

#[test]
fn export_created_during_configuration_is_checked_after_workspace_claim() {
    use horizon_app_process::storage::Directory;
    use horizon_app_runtime::journal::{Journal, execution::Workspace};
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let held = Directory::open(&root).unwrap();
    let client: Client = serde_json::from_value(configuration(&root)).unwrap();
    let journal = Arc::new(Journal::open(&root, &crate::actor::tests::account()).unwrap());
    let name = "00000000000000000000000000000002";
    evidence_ready(&held, name).unwrap();
    let export = root.join(name).join("crash-frame.png");
    assert_eq!(
        claim(&client, Arc::clone(&journal), &held, name, || {
            std::fs::write(&export, b"created after preliminary check").unwrap();
        })
        .err(),
        Some(Error::CleanupUncertain)
    );
    assert_eq!(std::fs::read(&export).unwrap(), b"created after preliminary check");
    let owned = Workspace::open(Arc::clone(&journal), client.owner, &root).unwrap();
    assert_eq!(
        claim(&client, journal, &held, name, || ()).err(),
        Some(Error::Runtime(horizon_app_runtime::Error::ExecutionBusy))
    );
    drop(owned);
}

#[test]
fn undispatched_preparation_reconciles_but_nonempty_local_state_stays_held() {
    use horizon_app_runtime::journal::{Journal, Kind, Phase, execution::Workspace};
    use std::time::Duration;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let journal = Arc::new(Journal::open(&root.join("private"), &crate::actor::tests::account()).unwrap());
    let owner = Uuid::new_v4();
    let workspace = Workspace::open(journal.clone(), owner, &root).unwrap();
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    for kind in [Kind::Upload, Kind::Session, Kind::Run, Kind::Tunnel] {
        let op = workspace.start(kind, Duration::from_secs(60)).unwrap();
        close_undispatched(&workspace, &op, &state).unwrap();
        assert_eq!(journal.status(owner, op.id).unwrap().phase, Phase::Complete);
        assert!(!state.join(op.id.simple().to_string()).exists());
    }
    let op = workspace.start(Kind::Run, Duration::from_secs(60)).unwrap();
    let directory = state.join(op.id.simple().to_string());
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(directory.join("unexpected"), b"retained").unwrap();
    assert_eq!(
        close_undispatched(&workspace, &op, &state),
        Err(Error::CleanupUncertain)
    );
    assert_eq!(std::fs::read(directory.join("unexpected")).unwrap(), b"retained");
    assert_eq!(journal.status(owner, op.id).unwrap().phase, Phase::Preparing);
}
