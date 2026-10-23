use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
#[cfg(target_os = "linux")]
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, time::SystemTime};

#[cfg(target_os = "linux")]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct StateEntry {
    inode: u64,
    mode: u32,
    modified: SystemTime,
    bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
pub(crate) fn state_snapshot(root: &Path) -> BTreeMap<PathBuf, StateEntry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, StateEntry>) {
        let metadata = std::fs::metadata(path).unwrap();
        entries.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            StateEntry {
                inode: metadata.ino(),
                mode: metadata.mode(),
                modified: metadata.modified().unwrap(),
                bytes: if metadata.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            },
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

const TUNNEL_CONTRACT: &str = "```yaml\nremote-device-testing:\n  version: 1\n  provider: browserstack\n  apps:\n    ios:\n      build: [build]\n      artifact: App.ipa\n      bundle_id: com.example.app\n  tunnel:\n    ports: {backend: 8080}\n  matrix: [{platform: ios, form: phone}]\n  recipes: [recipe.md]\n```";

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

#[test]
// Explicit reboot confirmations require the Linux kernel boot identity.
#[cfg(target_os = "linux")]
fn reboot_reconciliation_never_initializes_missing_state_or_owner_binding() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let account = crate::actor::tests::account();
    let confirmation = crate::local::recovery::RebootConfirmation::new(
        horizon_app_process::boot::current().unwrap(),
        vec![Uuid::new_v4()],
    )
    .unwrap();
    let mode = ReconciliationMode::for_confirmation(Some(&confirmation));
    let mut client: Client = serde_json::from_value(configuration(&root)).unwrap();
    client.state = root.join("missing-state");
    let before = state_snapshot(&root);
    assert!(mode.journal(&client.state, &account).is_err());
    assert_eq!(state_snapshot(&root), before);

    let journal = Arc::new(ReconciliationMode::Normal.journal(&client.state, &account).unwrap());
    let before = state_snapshot(&root);
    assert_eq!(
        mode.workspace(journal, &client).err(),
        Some(horizon_app_runtime::Error::OwnershipRefused)
    );
    assert_eq!(state_snapshot(&root), before);
}

#[test]
// Explicit reboot confirmations require the Linux kernel boot identity.
#[cfg(target_os = "linux")]
fn normal_reconciliation_can_initialize_but_reboot_reuses_existing_state_without_writes() {
    use horizon_app_runtime::journal::Kind;
    use std::time::Duration;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let account = crate::actor::tests::account();
    let mut client: Client = serde_json::from_value(configuration(&root)).unwrap();
    client.state = root.join("state");
    let normal = ReconciliationMode::for_confirmation(None);
    let journal = Arc::new(normal.journal(&client.state, &account).unwrap());
    let workspace = normal.workspace(journal, &client).unwrap();
    let operation = workspace.start(Kind::Run, Duration::from_secs(60)).unwrap();
    drop(workspace);

    let confirmation = crate::local::recovery::RebootConfirmation::new(
        horizon_app_process::boot::current().unwrap(),
        vec![operation.id],
    )
    .unwrap();
    let mode = ReconciliationMode::for_confirmation(Some(&confirmation));
    let before = state_snapshot(&root);
    let journal = Arc::new(mode.journal(&client.state, &account).unwrap());
    let workspace = mode.workspace(journal, &client).unwrap();
    assert_eq!(workspace.journal().pending(client.owner).unwrap()[0].id, operation.id);
    drop(workspace);
    assert_eq!(state_snapshot(&root), before);
}

fn credential_profile() -> horizon_browser::remote::RemoteProviderProfile {
    use horizon_browser::remote::{
        ControlEndpoint, CredentialReference, RemoteAdapterKind, RemoteAuthentication, RemoteSessionLimits,
    };
    horizon_browser::remote::RemoteProviderProfile {
        adapter: RemoteAdapterKind::Browserstack,
        endpoint: ControlEndpoint::parse("https://hub-cloud.browserstack.com/wd/hub").unwrap(),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings: std::collections::BTreeMap::new(),
        limits: RemoteSessionLimits::default(),
    }
}

fn private_file(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn credential_files(root: &Path, names: &[&str]) -> serde_json::Value {
    names
        .iter()
        .map(|name| ((*name).to_owned(), serde_json::json!(root.join(name))))
        .collect()
}

#[test]
fn credential_files_form_the_account_without_opening_the_os_keyring() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    private_file(&root.join("user"), b"synthetic-user\n");
    private_file(&root.join("key"), b"synthetic-key");
    let mut value = configuration(&root);
    value["credential_files"] = credential_files(&root, &["user", "key"]);
    let path = root.join("client.json");
    private_file(&path, value.to_string().as_bytes());
    let selected = client(&path).unwrap();
    let opened = std::cell::Cell::new(false);
    let keyring = || {
        opened.set(true);
        Err(horizon_app_runtime::Error::CredentialsUnavailable.into())
    };
    account(&selected, &credential_profile(), keyring).unwrap();
    assert!(!opened.get());

    let without: Client = serde_json::from_value(configuration(&root)).unwrap();
    assert!(account(&without, &credential_profile(), keyring).is_err());
    assert!(opened.get());
}

#[test]
fn credential_files_refuse_shared_foreign_linked_oversized_missing_and_extra_files() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let profile = credential_profile();
    let owner = rustix::process::geteuid().as_raw();
    private_file(&root.join("user"), b"synthetic-user");
    private_file(&root.join("key"), b"synthetic-key");
    let read = |names: &[&str], owner| {
        let files = serde_json::from_value(credential_files(&root, names)).unwrap();
        file_authorization(&profile, &files, owner)
    };
    let both = ["user", "key"];
    let unavailable = Err(Error::Runtime(horizon_app_runtime::Error::CredentialsUnavailable));
    let invalid = Err(Error::Runtime(horizon_app_runtime::Error::CredentialsInvalid));
    read(&both, owner).unwrap();

    std::fs::set_permissions(root.join("key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(read(&both, owner), unavailable);
    std::fs::set_permissions(root.join("key"), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read(&both, owner + 1), unavailable);

    std::fs::rename(root.join("key"), root.join("target")).unwrap();
    symlink(root.join("target"), root.join("key")).unwrap();
    assert_eq!(read(&both, owner), unavailable);
    std::fs::remove_file(root.join("key")).unwrap();
    assert_eq!(read(&both, owner), unavailable);
    private_file(&root.join("key"), &[b'k'; 4097]);
    assert_eq!(read(&both, owner), unavailable);
    private_file(&root.join("key"), b"synthetic\x07key");
    assert_eq!(read(&both, owner), invalid);

    private_file(&root.join("key"), b"synthetic-key");
    private_file(&root.join("extra"), b"synthetic-extra");
    assert_eq!(read(&["user"], owner), invalid);
    assert_eq!(read(&["user", "key", "extra"], owner), invalid);

    let mut relative = configuration(&root);
    relative["credential_files"] = serde_json::json!({"user": "user", "key": root.join("key")});
    let path = root.join("client.json");
    private_file(&path, relative.to_string().as_bytes());
    assert!(client(&path).is_err());
}

#[test]
fn client_with_tunnel_ports_needs_a_tunnel_binary_and_both_pin_fields() {
    use horizon_app_testing::contract::Contract;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let pinned: Client = serde_json::from_value(configuration(&root)).unwrap();
    let mut value = configuration(&root);
    let fields = value.as_object_mut().unwrap();
    fields.remove("tunnel_binary");
    fields.remove("tunnel_sha256");
    let unpinned: Client = serde_json::from_value(value.clone()).unwrap();
    let ports = Contract::from_agents(TUNNEL_CONTRACT).unwrap();
    let mut none = ports.clone();
    none.tunnel = None;
    assert!(tunnel_pin(&pinned, &ports).unwrap().is_some());
    assert!(matches!(tunnel_pin(&unpinned, &ports), Err(Error::TunnelUnconfigured)));
    assert!(tunnel_pin(&unpinned, &none).unwrap().is_none());

    let path = root.join("client.json");
    private_file(&path, value.to_string().as_bytes());
    client(&path).unwrap();
    for (field, half) in [
        ("tunnel_binary", "/synthetic/BrowserStackLocal".to_owned()),
        ("tunnel_sha256", "a".repeat(64)),
    ] {
        let mut one = value.clone();
        one[field] = half.into();
        private_file(&path, one.to_string().as_bytes());
        assert!(client(&path).is_err());
    }
}
