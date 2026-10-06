#![cfg(unix)]
use horizon_app_host::local::{Configuration, Lease, Local};
use horizon_app_process::{Event, Kind};
use horizon_app_runtime::{
    account::Account,
    journal::{Journal, Phase, execution::Workspace},
};
use horizon_browser::remote::*;
use horizon_core::remote_browser_credential::{
    CredentialLocator, CredentialStores, RemoteCredentialStore, SessionCredentialStore,
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[test]
fn cli_setup_failure_reports_a_typed_progress_error_without_private_paths() {
    let temp = tempfile::tempdir().unwrap();
    let client = temp.path().join("private-client-does-not-exist.json");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_horizon-native"))
        .args(["--run", "--client"])
        .arg(&client)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    let event: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(event["phase"], "error");
    assert_eq!(event["message"], horizon_app_host::Error::Unavailable.to_string());
    assert!(
        !String::from_utf8(result.stderr)
            .unwrap()
            .contains(&client.display().to_string())
    );
}

fn account() -> Account {
    let bindings = ["user", "key"]
        .map(|reference| {
            (
                CredentialReference::from(reference),
                CredentialBinding {
                    store: CredentialStoreKind::Session,
                    slot: None,
                },
            )
        })
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let profile = RemoteProviderProfile {
        adapter: RemoteAdapterKind::Browserstack,
        endpoint: ControlEndpoint::parse("https://hub-cloud.browserstack.com/wd/hub").unwrap(),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings: bindings,
        limits: RemoteSessionLimits::default(),
    };
    let mut store = SessionCredentialStore::new();
    for (reference, value) in [("user", "synthetic-user"), ("key", "synthetic-key")] {
        let reference = CredentialReference::from(reference);
        store
            .put(
                &CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference]),
                value.as_bytes(),
            )
            .unwrap();
    }
    Account::capture(
        &profile,
        &CredentialStores {
            session: &store,
            os_keychain: None,
            environment: None,
        },
    )
    .unwrap()
}
fn fixture() -> (tempfile::TempDir, Arc<Workspace>, Local) {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    let account = account();
    let journal = Arc::new(Journal::open(&root.join("journal"), &account).unwrap());
    let workspace = Arc::new(Workspace::open(journal, Uuid::new_v4(), &root).unwrap());
    let state = root.join("processes");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let worker = Path::new(env!("CARGO_BIN_EXE_horizon-native"));
    let local = Local::new(
        Arc::clone(&workspace),
        &account,
        &root,
        Configuration {
            process_worker: worker.into(),
            tunnel_worker: worker.into(),
            tunnel_binary: root.join("unused"),
            tunnel_sha256: "a".repeat(64),
            state,
        },
    )
    .unwrap();
    (temp, workspace, local)
}
fn journal_directory(root: &Path) -> std::path::PathBuf {
    std::fs::read_dir(root.join("journal"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("journal.json").is_file())
        .unwrap()
}
fn command() -> Vec<String> {
    vec!["python3".into(), "-c".into(), "import json,socket,sys,signal; signal.signal(signal.SIGTERM,lambda *args:None); s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(json.dumps({'native_backend_ready':1,'port':s.getsockname()[1]}),flush=True); request=json.loads(sys.stdin.readline()); sys.stdin.read(); s.close(); print(json.dumps({'native_backend_closed':1,'nonce':request['nonce']}),flush=True)".into()]
}
fn start(local: &Local) -> (Lease, u16) {
    let id = local
        .process(
            command(),
            Kind::Backend,
            Duration::from_secs(5),
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(matches!(id.next(Duration::from_secs(5)).unwrap(), Event::Started {}));
    match id.next(Duration::from_secs(5)).unwrap() {
        Event::Ready { port } => (id, port),
        _ => panic!("backend readiness missing"),
    }
}
#[test]
fn closing_one_host_lane_keeps_the_other_backend_and_journal_active() {
    let (_root, workspace, local) = fixture();
    let (first, a) = start(&local);
    let (second, b) = start(&local);
    assert_ne!(a, b);
    first.close().unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", a)).is_err());
    assert!(std::net::TcpStream::connect(("127.0.0.1", b)).is_ok());
    assert_eq!(
        workspace.journal().status(workspace.owner(), first.id()).unwrap().phase,
        Phase::Complete
    );
    assert_eq!(
        workspace
            .journal()
            .status(workspace.owner(), second.id())
            .unwrap()
            .phase,
        Phase::Active
    );
    second.close().unwrap();
}
#[test]
fn host_drop_closes_its_guardian_and_promptly_releases_execution_ownership() {
    let (root, workspace, local) = fixture();
    let (id, port) = start(&local);
    let owner = workspace.owner();
    let journal = Arc::new(Journal::open(&root.path().canonicalize().unwrap().join("journal"), &account()).unwrap());
    let operation = id.id();
    drop(id);
    drop(local);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    assert_eq!(
        workspace.journal().status(owner, operation).unwrap().phase,
        Phase::Complete
    );
    drop(workspace);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Workspace::open(Arc::clone(&journal), owner, &root.path().canonicalize().unwrap()) {
            Ok(_) => break,
            Err(horizon_app_runtime::Error::ExecutionBusy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("execution ownership was retained after cleanup: {error}"),
        }
    }
}

#[test]
fn journal_failure_does_not_prevent_stopping_the_exact_retained_backend() {
    let (root, _workspace, local) = fixture();
    let (id, port) = start(&local);
    std::fs::remove_file(journal_directory(root.path()).join("journal.json")).unwrap();
    assert!(matches!(id.close(), Err(horizon_app_host::Error::CleanupUncertain)));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}
#[test]
fn known_no_spawn_failures_complete_and_retire_old_private_directories() {
    let (root, workspace, local) = fixture();
    for _ in 0..40 {
        assert!(matches!(
            local.process(Vec::new(), Kind::Build, Duration::from_secs(5), Duration::from_secs(30)),
            Err(horizon_app_host::Error::Process(horizon_app_process::Error::Invalid))
        ));
    }
    assert!(workspace.journal().pending(workspace.owner()).unwrap().is_empty());
    assert!(workspace.journal().completed(workspace.owner()).unwrap().len() <= 33);
    assert!(std::fs::read_dir(root.path().join("processes")).unwrap().count() <= 33);
}
#[test]
fn completed_state_retirement_never_follows_a_link_into_other_data() {
    use std::os::unix::fs::symlink;
    let (root, _workspace, _local) = fixture();
    let directory =
        horizon_app_process::storage::Directory::open(&root.path().canonicalize().unwrap().join("processes")).unwrap();
    let name = Uuid::new_v4().simple().to_string();
    directory.create_child(&name).unwrap();
    let outside = root.path().join("unrelated");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"unrelated").unwrap();
    symlink(&outside, root.path().join("processes").join(&name).join("link")).unwrap();
    directory.retire_child(&name).unwrap();
    assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"unrelated");
    directory.retire_child(&name).unwrap();
    assert!(directory.retire_child("../unrelated").is_err());
}

#[test]
fn delayed_admission_and_arming_cannot_authorize_a_command_after_the_original_deadline() {
    use std::os::unix::fs::PermissionsExt;
    let (root, workspace, mut local) = fixture();
    let worker = root.path().join("delayed-guardian.py");
    std::fs::write(
        &worker,
        r"#!/usr/bin/python3
import json,os,pathlib,sys,time
spec=json.loads(sys.stdin.readline())
state=pathlib.Path(spec['state'])
receipt={'operation':spec['operation'],'guardian_pid':os.getpid(),'complete':False}
path=state/'process.json'
path.write_text(json.dumps(receipt));path.chmod(0o600)
time.sleep(3)
print(json.dumps({'phase':'armed'}),flush=True)
start=sys.stdin.readline()
if start=='start\n':
    (pathlib.Path(spec['root'])/'forbidden').write_text('executed after expiry')
receipt['complete']=True
path.write_text(json.dumps(receipt))
if start=='start\n':print(json.dumps({'phase':'complete','success':True}),flush=True)
",
    )
    .unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Use a second host configuration with the same trusted workspace but a deliberately delayed private worker.
    // No existing lease has started yet; the fixture's default actor is dropped first.
    drop(local);
    let account = account();
    let state = root.path().canonicalize().unwrap().join("processes");
    local = Local::new(
        Arc::clone(&workspace),
        &account,
        &root.path().canonicalize().unwrap(),
        Configuration {
            process_worker: worker.clone(),
            tunnel_worker: worker,
            tunnel_binary: root.path().join("unused"),
            tunnel_sha256: "a".repeat(64),
            state,
        },
    )
    .unwrap();
    let lock_path = journal_directory(root.path()).join("journal.lock");
    let lock = std::fs::File::open(lock_path).unwrap();
    lock.lock().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        lock.unlock().unwrap();
    });
    let result = local.process(
        vec!["touch".into(), "forbidden".into()],
        Kind::Build,
        Duration::from_secs(1),
        Duration::from_secs(5),
    );
    release.join().unwrap();
    assert!(result.is_err());
    assert!(!root.path().join("forbidden").exists());
    assert!(workspace.journal().pending(workspace.owner()).unwrap().is_empty());
    let completed = workspace.journal().completed(workspace.owner()).unwrap();
    assert_eq!(completed.len(), 1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        now < completed[0].deadline_seconds,
        "regression must reach the expired monotonic deadline while the later journal deadline is still live"
    );
}

#[test]
fn cleanup_retry_accepts_only_owned_durably_complete_resources_after_in_memory_pruning() {
    let (_root, workspace, local) = fixture();
    let (first, _) = start(&local);
    first.close().unwrap();
    let (second, _) = start(&local); // Starting another operation prunes completed in-memory leases.
    assert_eq!(
        workspace.journal().status(workspace.owner(), first.id()).unwrap().phase,
        Phase::Complete
    );
    first.close().unwrap();
    assert!(local.recover(Uuid::new_v4()).is_err());
    second.close().unwrap();
}
