//! Opt-in transport component: original task allocation and pins, never native approval.
use super::super::*;
use crate::cloud_runtime::{
    command::Runner,
    local_network::endpoint::{Attempt, Binding, Endpoint, Prepared, SourceKind},
    settings::Settings,
    state::{Deployment, Store},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::{File, Metadata, OpenOptions, TryLockError},
    io::Read,
    net::{Ipv4Addr, SocketAddrV4},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const DIRECTORY: &str = "/run/horizon-m1-ssh-qualification";
const PORT: u16 = 18080;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    root_authorized_disposable_component: bool,
    task_only_fresh_ssh_identity: bool,
    task_ssh_identity_file: PathBuf,
    cloud_root: PathBuf,
    settings_path: PathBuf,
    original_owner_uid: u32,
    cloud_id: String,
    worker_id: String,
    operation_id: String,
    image_digest: String,
    deployment_sha256: String,
    settings_sha256: String,
    known_hosts_sha256: String,
    target: Target,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Tailnet {
        desktop_id: String,
        peer_id: String,
        peer_name: String,
        peer_ipv4: Ipv4Addr,
    },
    Lan {
        host: String,
        address: Ipv4Addr,
        network: String,
        desktop: Ipv4Addr,
        interface: String,
    },
}

impl Target {
    fn source(&self) -> SourceKind {
        match self {
            Self::Tailnet { .. } => SourceKind::Tailnet,
            Self::Lan { .. } => SourceKind::Lan,
        }
    }
    fn address(&self) -> Ipv4Addr {
        match self {
            Self::Tailnet { peer_ipv4, .. } => *peer_ipv4,
            Self::Lan { address, .. } => *address,
        }
    }
    fn prepared(&self) -> Prepared {
        match self {
            Self::Tailnet {
                desktop_id,
                peer_id,
                peer_name,
                peer_ipv4,
            } => {
                let prepared = Prepared::tailnet(Endpoint::new(peer_name, PORT).expect("original peer endpoint"))
                    .expect("production kernel route/Self/WhoIs/name checks");
                assert!(
                    matches!(&prepared.binding, Binding::Tailnet { desktop, peer, name, address }
                    if desktop == desktop_id && peer == peer_id && name == peer_name
                    && *address == (*peer_ipv4, PORT).into())
                );
                prepared
            }
            Self::Lan {
                host,
                address,
                network,
                desktop,
                interface,
            } => {
                let prepared = Prepared::lan(Endpoint::new(host, PORT).expect("original LAN endpoint"))
                    .expect("production current LAN policy and route checks");
                assert!(
                    matches!(&prepared.binding, Binding::Lan { network: found, address: bound }
                    if found.subnet.to_string() == *network && found.address == *desktop
                    && found.interface == *interface && *bound == (*address, PORT).into())
                );
                prepared
            }
        }
    }
}

fn parent_allowed(path: &Path, uid: u32, mode: u32, owner: u32) -> bool {
    (uid == 0 || uid == owner)
        && (mode & 0o022 == 0 || (path == Path::new("/tmp") && uid == 0 && mode & 0o7777 == 0o1777))
}

fn private_path(path: &Path, owner: u32) -> Metadata {
    assert!(path.is_absolute());
    assert_eq!(path.canonicalize().expect("existing canonical original path"), path);
    for part in path.ancestors() {
        let meta = std::fs::symlink_metadata(part).expect("original component path required");
        assert!(!meta.file_type().is_symlink());
        if part != path {
            assert!(meta.is_dir() && parent_allowed(part, meta.uid(), meta.mode(), owner));
        }
    }
    std::fs::symlink_metadata(path).expect("private original metadata")
}

fn file_allowed(meta: &Metadata, owner: u32) -> bool {
    meta.is_file() && meta.uid() == owner && meta.mode() & 0o7777 == 0o600 && meta.nlink() == 1
}

fn private_metadata(path: &Path, owner: u32) -> Metadata {
    let meta = private_path(path, owner);
    assert!(
        file_allowed(&meta, owner),
        "exact original owner/private regular file required"
    );
    meta
}

fn same_file(before: &Metadata, after: &Metadata) {
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
}

fn private_open(path: &Path, owner: u32, write: bool) -> File {
    let before = private_metadata(path, owner);
    let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).expect("portable NOFOLLOW flag");
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(nofollow)
        .open(path)
        .expect("exact original file open");
    check_opened(path, owner, &before, &file);
    file
}

fn check_opened(path: &Path, owner: u32, before: &Metadata, file: &File) {
    let opened = file.metadata().expect("opened original metadata");
    assert!(file_allowed(&opened, owner));
    same_file(before, &opened);
    same_file(&opened, &private_metadata(path, owner));
}

fn private_file(path: &Path, owner: u32, limit: u64) -> Vec<u8> {
    let mut file = private_open(path, owner, false);
    let before = file.metadata().expect("opened original metadata");
    assert!(before.len() <= limit);
    let mut data = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut data)
        .expect("bounded original input");
    assert!(data.len() as u64 <= limit);
    assert!(file_allowed(&file.metadata().expect("final opened metadata"), owner));
    same_file(&before, &private_metadata(path, owner));
    data
}

fn checked(path: &Path, owner: u32, limit: u64, expected: &str) -> Vec<u8> {
    assert_eq!(expected.len(), 64);
    let bytes = private_file(path, owner, limit);
    assert_eq!(sha256(&bytes), expected);
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            write!(text, "{byte:02x}").expect("format digest into a String");
            text
        })
}

fn original_store(root: &Path, owner: u32, cloud_id: &str) -> Store {
    assert_eq!(root.file_name().and_then(|name| name.to_str()), Some(cloud_id));
    let directory = private_path(root, owner);
    assert!(directory.is_dir() && directory.uid() == owner && directory.mode() & 0o7777 == 0o700);
    let path = root.join("operation.lock");
    let witness = private_open(&path, owner, true);
    let store = Store::lock_existing(root).expect("original allocation lifecycle lock");
    same_file(&witness.metadata().unwrap(), &private_metadata(&path, owner));
    assert!(
        matches!(witness.try_lock(), Err(TryLockError::WouldBlock)),
        "Store must hold the original inode"
    );
    store
}

fn selected_key(settings: &Settings, original: &Path, owner: u32) -> Metadata {
    assert_eq!(settings.ssh_identity_file, original);
    private_metadata(original, owner)
}

struct Original {
    store: Store,
    state: Deployment,
    connection: Connection,
    key_metadata: Metadata,
}

impl Original {
    fn checked(fixture: &Fixture) -> Self {
        let owner = fixture.original_owner_uid;
        let store = original_store(&fixture.cloud_root, owner, &fixture.cloud_id);
        let deployment = fixture.cloud_root.join("deployment.json");
        checked(&deployment, owner, 512 * 1024, &fixture.deployment_sha256);
        let state = store
            .load()
            .expect("original settled record")
            .expect("original allocation");
        checked(&deployment, owner, 512 * 1024, &fixture.deployment_sha256);
        assert!(state.worker_ready(), "original worker must already be Ready and bound");
        assert_eq!(state.cloud_id, fixture.cloud_id);
        let worker = state.worker.as_ref().expect("original worker");
        let spec = state.spec.as_ref().expect("original allocation specification");
        assert_eq!(worker.id, fixture.worker_id);
        assert_eq!(spec.operation_id, fixture.operation_id);
        assert_eq!(spec.image_digest, fixture.image_digest);
        assert_eq!(worker.image_name, fixture.image_digest);
        let bytes = checked(&fixture.settings_path, owner, 256 * 1024, &fixture.settings_sha256);
        let settings: Settings = serde_json::from_slice(&bytes).expect("exact hashed original Settings");
        settings.validate().expect("production Settings validation");
        let key_metadata = selected_key(&settings, &fixture.task_ssh_identity_file, owner);
        let connection = Connection::new(worker, &settings, store.root()).expect("original SSH route and pins");
        checked(&connection.known_hosts, owner, 64 * 1024, &fixture.known_hosts_sha256);
        Self {
            store,
            state,
            connection,
            key_metadata,
        }
    }
    fn unchanged(&self, fixture: &Fixture) {
        let owner = fixture.original_owner_uid;
        checked(
            &fixture.cloud_root.join("deployment.json"),
            owner,
            512 * 1024,
            &fixture.deployment_sha256,
        );
        checked(&fixture.settings_path, owner, 256 * 1024, &fixture.settings_sha256);
        checked(
            &self.connection.known_hosts,
            owner,
            64 * 1024,
            &fixture.known_hosts_sha256,
        );
        same_file(
            &self.key_metadata,
            &private_metadata(&fixture.task_ssh_identity_file, owner),
        );
        assert_eq!(
            self.store.load().unwrap().unwrap().worker.unwrap().id,
            fixture.worker_id
        );
    }
}

fn worker_status(connection: &Connection, runner: &Runner<'_>) -> bool {
    let bytes = runner
        .private_exchange(
            &mut connection.pinned_command("horizon-cloud-worker local-network status"),
            b"",
            Duration::from_secs(10),
        )
        .expect("bounded status from original pinned worker");
    let status: serde_json::Value = serde_json::from_slice(&bytes).expect("actual worker status JSON");
    status["active"].as_bool().expect("actual worker active flag")
}

// Fixed program, bounded request/reply and no credentials on the worker. The target
// comes from the production checked binding; the proxy comes from actual helper Ready.
const PROBE: &str = r"import json,socket,struct,sys
p=json.load(sys.stdin)
def exact(s,n):
 b=b''
 while len(b)<n:
  c=s.recv(n-len(b)); assert c; b+=c
 return b
def connect(host,port):
 s=socket.create_connection(('127.0.0.1',p['proxy_port']),5); s.settimeout(5)
 s.sendall(bytes([5,1,0])); assert exact(s,2)==bytes([5,0])
 s.sendall(bytes([5,1,0,1])+socket.inet_aton(host)+struct.pack('!H',port))
 reply=exact(s,10); assert reply[0]==5
 return s,reply[1]
s,code=connect(p['peer_ip'],18080); assert code==0
s.sendall(b'M1-REAL-KERNEL-REQUEST\n'); s.shutdown(socket.SHUT_WR)
assert exact(s,20)==b'M1-REAL-KERNEL-PEER\n'; assert s.recv(1)==b''; s.close()
s,code=connect(p['peer_ip'],18081); assert code==2; s.close()
s,code=connect('127.0.0.1',18080); assert code==2; s.close()
print(json.dumps({'allowed_bytes_after_eof':True,'wrong_port_denied':True,'wrong_destination_denied':True}))
";

#[derive(Serialize)]
struct ProbeInput {
    proxy_port: u16,
    peer_ip: Ipv4Addr,
}

fn record_attempt(directory: &Path, fixture: &Fixture, state: &Deployment, attempt: Attempt) {
    let worker = state.worker.as_ref().expect("original worker");
    let spec = state.spec.as_ref().expect("original allocation specification");
    let mut intent = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join("component-attempt.json"))
        .expect("fresh durable component attempt required");
    serde_json::to_writer(
        &mut intent,
        &serde_json::json!({
            "attempt": attempt, "cloud_id": state.cloud_id, "worker_id": worker.id,
            "operation_id": spec.operation_id, "deployment_sha256": fixture.deployment_sha256,
            "target": &fixture.target, "port": PORT,
            "native_owner_qualification": false
        }),
    )
    .expect("bounded component provenance");
    intent.sync_all().expect("component attempt durability");
    File::open(directory)
        .expect("owned fixture directory")
        .sync_all()
        .expect("attempt directory durability");
}

fn wait_ready(attached: &Attached) -> SocketAddrV4 {
    let deadline = Instant::now() + Duration::from_secs(75);
    loop {
        assert!(Instant::now() < deadline, "actual helper Ready deadline");
        if let local_network::State::Active { proxy } = attached.status().worker {
            break proxy;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_retired(connection: &Connection, runner: &Runner<'_>) {
    let deadline = Instant::now() + Duration::from_secs(75);
    while worker_status(connection, runner) {
        assert!(Instant::now() < deadline, "original remote helper must retire");
        thread::sleep(Duration::from_millis(100));
    }
}

fn run_original_worker(directory: &Path, source: SourceKind) {
    let fixture: Fixture = serde_json::from_slice(&private_file(&directory.join("input.json"), 0, 8192))
        .expect("explicit Root component intent");
    assert!(fixture.root_authorized_disposable_component && fixture.task_only_fresh_ssh_identity);
    assert_eq!(fixture.target.source(), source);
    let original = Original::checked(&fixture);
    let connection = &original.connection;
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(
        !worker_status(connection, &runner),
        "never replace a preexisting bridge"
    );
    let prepared = fixture.target.prepared();
    let attempt = prepared.attempt();
    record_attempt(directory, &fixture, &original.state, attempt);
    let gateway = Gateway::start_private(prepared).expect("production source recheck and private listener");
    let private_path = gateway.proxy.private_path().expect("private Unix endpoint").to_owned();
    let attached = gateway
        .attach(connection.clone())
        .expect("actual production pinned SSH attach");
    let proxy = wait_ready(&attached);
    assert!(proxy.ip().is_loopback());
    let receipt = attached.status().gateway.receipt;
    assert_eq!(receipt.operation_id, attempt.operation_id);
    assert_eq!(receipt.generation, attempt.generation);
    let request = serde_json::to_vec(&ProbeInput {
        proxy_port: proxy.port(),
        peer_ip: fixture.target.address(),
    })
    .expect("bounded nonsecret probe input");
    let remote = format!("python3 -c '{}'", PROBE.replace('\'', "'\\''"));
    let bytes = runner
        .private_exchange(
            &mut connection.pinned_command(&remote),
            &request,
            Duration::from_secs(25),
        )
        .expect("actual worker SOCKS relay must succeed");
    let result: serde_json::Value = serde_json::from_slice(&bytes).expect("typed probe result");
    assert_eq!(
        result,
        serde_json::json!({"allowed_bytes_after_eof":true,"wrong_port_denied":true,"wrong_destination_denied":true})
    );
    let after = fixture.target.prepared();
    assert_eq!(after.binding, attached.gateway.gate.prepared.binding);
    attached.close(&receipt).expect("close exact original receipt");
    assert!(matches!(attached.status().gateway.state, State::Revoked { .. }));
    drop(attached);
    assert!(!private_path.exists(), "original private listener custody retired");
    wait_retired(connection, &runner);
    original.unchanged(&fixture);
    println!(
        "M1_REAL_SSH_RECEIPT {}",
        serde_json::json!({
            "source": source, "passed": true, "actual_pinned_ssh": true,
            "peer_bytes_after_eof": true, "wrong_port_denied": true,
            "wrong_destination_denied": true, "exact_close": true,
            "original_worker_retained": true, "native_owner_qualification": false
        })
    );
}

#[test]
#[ignore = "Root GO only: disposable kernel guest plus original task-owned CPU worker/Store/Settings/pins; no host run"]
fn real_original_worker_private_ssh_relays_exact_peer_and_closes() {
    run_original_worker(&Path::new(DIRECTORY).join("tailnet"), SourceKind::Tailnet);
}

#[test]
#[ignore = "Root GO only: owned real LAN peer and original task-owned CPU worker/Store/Settings/pins; no host run"]
fn real_original_worker_private_ssh_relays_exact_lan_and_closes() {
    run_original_worker(&Path::new(DIRECTORY).join("lan"), SourceKind::Lan);
}

#[test]
fn original_owner_policy_does_not_grant_control_or_other_user_access() {
    assert!(parent_allowed(Path::new("/private"), 1000, 0o700, 1000));
    assert!(parent_allowed(Path::new("/private"), 0, 0o755, 1000));
    assert!(parent_allowed(Path::new("/tmp"), 0, 0o1777, 1000));
    assert!(!parent_allowed(Path::new("/private"), 1000, 0o720, 1000));
    assert!(!parent_allowed(Path::new("/private"), 2000, 0o700, 1000));
    for (path, uid, mode) in [
        ("/private", 2000, 0o700),
        ("/private", 1000, 0o720),
        ("/other", 0, 0o1777),
        ("/tmp", 1000, 0o1777),
        ("/private", 1000, 0o700),
    ] {
        assert!(!parent_allowed(Path::new(path), uid, mode, 0));
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    let meta = file.as_file().metadata().unwrap();
    assert!(file_allowed(&meta, meta.uid()));
    assert!(!file_allowed(&meta, meta.uid().wrapping_add(1)));
    if meta.uid() != 0 {
        assert!(!file_allowed(&meta, 0));
    }
}

fn private_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn original_file_refuses_symlink_hardlink_permissions_and_wrong_uid() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = private_directory();
    let path = directory.path().join("original");
    std::fs::write(&path, b"synthetic record").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let owner = path.metadata().unwrap().uid();
    assert_eq!(private_file(&path, owner, 64), b"synthetic record");
    assert!(std::panic::catch_unwind(|| private_file(&path, owner.wrapping_add(1), 64)).is_err());
    let alias = directory.path().join("alias");
    symlink(&path, &alias).unwrap();
    assert!(std::panic::catch_unwind(|| private_file(&alias, owner, 64)).is_err());
    std::fs::remove_file(&alias).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(std::panic::catch_unwind(|| private_file(&path, owner, 64)).is_err());
    std::fs::remove_file(&alias).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o620)).unwrap();
    assert!(std::panic::catch_unwind(|| private_file(&path, owner, 64)).is_err());
}

#[test]
fn opened_original_rechecks_inode_and_permissions_after_open() {
    use std::os::unix::fs::PermissionsExt;
    let first = tempfile::NamedTempFile::new().unwrap();
    let second = tempfile::NamedTempFile::new().unwrap();
    let before = first.as_file().metadata().unwrap();
    let owner = before.uid();
    check_opened(first.path(), owner, &before, first.as_file());
    assert!(std::panic::catch_unwind(|| check_opened(first.path(), owner, &before, second.as_file())).is_err());
    std::fs::set_permissions(first.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(std::panic::catch_unwind(|| check_opened(first.path(), owner, &before, first.as_file())).is_err());
}

#[test]
fn original_store_holds_the_original_lock_and_refuses_another_basename() {
    use std::os::unix::fs::PermissionsExt;
    let directory = private_directory();
    let root = directory.path().join("original");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let owner = root.metadata().unwrap().uid();
    let lock = root.join("operation.lock");
    assert!(std::panic::catch_unwind(|| original_store(&root, owner, "wrong")).is_err());
    assert!(!lock.exists(), "invalid original identity must not create a lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = original_store(&root, owner, "original");
    let witness = private_open(&lock, owner, true);
    assert!(matches!(witness.try_lock(), Err(TryLockError::WouldBlock)));
    drop(store);
    assert!(
        witness.try_lock().is_ok(),
        "the same original inode must become available"
    );
}

#[test]
fn hashed_settings_require_the_same_selected_key_without_reading_key_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = private_directory();
    let key = directory.path().join("task-only-key");
    std::fs::write(&key, b"synthetic non-key bytes").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "runpod_key_file":"/synthetic/absent", "ssh_identity_file":key,
        "docker_config":"/synthetic/docker", "registry_pull_auth_id":null,
        "cpu_flavors":[], "gpu_types":[]
    }))
    .unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    settings.validate().unwrap();
    let owner = key.metadata().unwrap().uid();
    selected_key(&settings, &key, owner);
    assert!(std::panic::catch_unwind(|| selected_key(&settings, &directory.path().join("successor"), owner)).is_err());
    let file = directory.path().join("settings.json");
    std::fs::write(&file, &bytes).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let hash = sha256(&bytes);
    assert_eq!(checked(&file, owner, 4096, &hash), bytes);
    std::fs::write(&file, b"changed original settings").unwrap();
    assert!(std::panic::catch_unwind(|| checked(&file, owner, 4096, &hash)).is_err());
}

#[test]
fn strict_target_metadata_keeps_actual_lan_and_tailnet_bindings_separate() {
    let tailnet: Target = serde_json::from_value(serde_json::json!({"source":"tailnet",
        "desktop_id":"self", "peer_id":"peer", "peer_name":"peer.test.ts.net", "peer_ipv4":"100.64.1.2"}))
    .unwrap();
    assert_eq!(tailnet.source(), SourceKind::Tailnet);
    let mut lan = serde_json::json!({"source":"lan", "host":"192.168.1.50", "address":"192.168.1.50",
        "network":"192.168.1.0/24", "desktop":"192.168.1.10", "interface":"fixture0"});
    let target: Target = serde_json::from_value(lan.clone()).unwrap();
    assert_eq!(target.source(), SourceKind::Lan);
    assert_eq!(target.address(), "192.168.1.50".parse::<Ipv4Addr>().unwrap());
    lan["peer_id"] = "must-not-substitute".into();
    assert!(serde_json::from_value::<Target>(lan).is_err());
}
