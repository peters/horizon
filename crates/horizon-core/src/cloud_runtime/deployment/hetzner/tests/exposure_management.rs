//! Opt-in provider qualification of the SDK, not Native or public exposure approval.
use super::super::{Compute, Journal as NormalJournal, JournalFile as _};
use crate::cloud_runtime::{
    Cancellation, CreateState, Error, Result,
    command::Runner,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, Store},
    tailnet::Selection,
};
use horizon_cloud::{
    CloudError,
    hetzner::firewalls::{Direction, Protocol, Rule, Scope, management},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::Write,
    net::IpAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const ROOT: &str = "/home/peters/github/horizon/.task-1335-native-evidence/sdk-exposure-management";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    cloud: PathBuf,
    settings: PathBuf,
    cloud_id: String,
    allocation: String,
    server: u64,
    volume: u64,
    key: u64,
    test_operation: String,
    nonce: String,
    management_source: IpAddr,
    settings_sha256: String,
    deployment_sha256: String,
    normal_journal_sha256: String,
    known_hosts_sha256: String,
    credential: Identity,
    ssh_key: Identity,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    path: PathBuf,
    device: u64,
    inode: u64,
    uid: u32,
    size: u64,
    mode: u32,
    changed: i64,
    changed_nsec: i64,
}
impl Identity {
    fn check(&self, path: &Path) -> Result<()> {
        let metadata = private_file(path, 64 * 1024)?;
        require(
            path == self.path && path.canonicalize()? == self.path,
            "Original credential path changed",
        )?;
        require(
            metadata.dev() == self.device
                && metadata.ino() == self.inode
                && metadata.uid() == self.uid
                && metadata.len() == self.size
                && metadata.mode() == self.mode
                && metadata.ctime() == self.changed
                && metadata.ctime_nsec() == self.changed_nsec,
            "Original credential metadata changed",
        )
    }
}
fn require(ok: bool, problem: &'static str) -> Result<()> {
    if ok { Ok(()) } else { Err(Error::Invalid(problem)) }
}
fn private_file(path: &Path, maximum: u64) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.len() > 0
            && metadata.len() <= maximum
            && metadata.permissions().mode().trailing_zeros() >= 6
            && path.canonicalize()? == path,
        "Qualification file is not original and private",
    )?;
    Ok(metadata)
}
fn digest_matches(path: &Path, expected: &str) -> Result<()> {
    private_file(path, 4 * 1024 * 1024)?;
    let bytes = fs::read(path)?;
    let mut actual = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(actual, "{byte:02x}").map_err(|_| Error::Invalid("Qualification digest encoding failed"))?;
    }
    require(actual == expected, "Original qualification input changed")
}
impl Input {
    fn load() -> Result<Self> {
        let root = Path::new(ROOT);
        let metadata = fs::symlink_metadata(root)?;
        require(
            metadata.is_dir() && metadata.permissions().mode().trailing_zeros() >= 6 && root.canonicalize()? == root,
            "Qualification root is not original and private",
        )?;
        let path = root.join("input.json");
        require(
            private_file(&path, 16 * 1024)?.uid() == metadata.uid(),
            "Qualification input owner changed",
        )?;
        let input: Self = serde_json::from_slice(&fs::read(path)?).map_err(|_| Error::Json)?;
        require(
            input.cloud.starts_with(root)
                && input.cloud != root
                && input.cloud.canonicalize()? == input.cloud
                && input.nonce.len() == 32
                && input.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
                && input.test_operation != input.allocation
                && input.server > 0
                && input.volume > 0
                && input.key > 0,
            "Qualification input does not identify a fresh normal worker",
        )?;
        input.check()?;
        Ok(input)
    }
    fn check(&self) -> Result<()> {
        self.credential.check(&self.credential.path)?;
        self.ssh_key.check(&self.ssh_key.path)?;
        digest_matches(&self.settings, &self.settings_sha256)?;
        digest_matches(&self.cloud.join("deployment.json"), &self.deployment_sha256)?;
        digest_matches(&self.cloud.join("hetzner.json"), &self.normal_journal_sha256)?;
        digest_matches(
            &self.cloud.join(format!("known-hosts-{}", self.server)),
            &self.known_hosts_sha256,
        )
    }
    fn ssh(&self, connection: &Connection, cancel: &Cancellation) -> Result<()> {
        self.check()?;
        let remote = format!(
            "printf '%s\n' 'horizon-sdk-{}'; printf '%s\n' \"$SSH_CONNECTION\"",
            self.nonce
        );
        let mut command = Command::new("/usr/bin/ssh");
        command
            .args([
                "-T",
                "-a",
                "-x",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ClearAllForwardings=yes",
            ])
            .args(connection.pinned_args())
            .arg(remote);
        let runner = Runner {
            cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let bytes = runner.private_exchange(&mut command, &[], Duration::from_secs(15))?;
        require(bytes.len() <= 1024, "Original SSH observation is oversized")?;
        let output = std::str::from_utf8(&bytes).map_err(|_| Error::PrivateTransport)?;
        let lines: Vec<_> = output.lines().collect();
        require(
            lines.len() == 2 && lines[0] == format!("horizon-sdk-{}", self.nonce),
            "Original SSH nonce changed",
        )?;
        let tuple: Vec<_> = lines[1].split_ascii_whitespace().collect();
        require(tuple.len() == 4, "Original SSH connection tuple is missing")?;
        require(
            tuple[0].parse::<IpAddr>().ok() == Some(self.management_source)
                && tuple[1].parse::<u16>().is_ok_and(|port| port > 0)
                && tuple[2].parse::<IpAddr>().is_ok()
                && tuple[3].parse::<u16>().ok() == Some(connection.port),
            "Original SSH management source changed",
        )
    }
}

/// The supervisor observes this same exclusive file, including intent before POST.
/// Partial writes remain failures and cannot restore a Manager after process loss.
struct Saved {
    file: File,
    entries: usize,
}
impl Saved {
    fn new(input: &Input) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(Path::new(ROOT).join("sdk-journal.jsonl"))?;
        let mut saved = Self { file, entries: 0 };
        saved.append(&json!({"scope":input.test_operation,"allocation":input.allocation,
            "server":input.server,"cloud":input.cloud}))?;
        File::open(ROOT)?.sync_all()?;
        Ok(saved)
    }
    fn append(&mut self, value: &Value) -> std::io::Result<()> {
        if self.entries >= 64 {
            return Err(std::io::Error::other("Qualification journal limit"));
        }
        let mut bytes = serde_json::to_vec(value)?;
        if bytes.len() > 64 * 1024 || self.file.metadata()?.len() + bytes.len() as u64 > 1024 * 1024 {
            return Err(std::io::Error::other("Qualification journal limit"));
        }
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.entries += 1;
        Ok(())
    }
}
impl management::Journal for Saved {
    fn save(&mut self, update: management::Update<'_>) -> std::result::Result<(), CloudError> {
        let value = match update {
            management::Update::Intent {
                kind,
                firewall,
                rules,
                foreign,
            } => json!({"intent":format!("{kind:?}"),"firewall":firewall,"rules":rules,"foreign":foreign}),
            management::Update::Reconciled { firewall } => json!({"reconciled":firewall}),
            management::Update::Actions { firewall, actions } => json!({"firewall":firewall,
                "actions":actions.iter().map(|action| json!({"id":action.id,"status":action.status})).collect::<Vec<_>>()}),
            management::Update::Confirmed { firewall, rules } => json!({"confirmed":firewall,"rules":rules}),
        };
        self.append(&value).map_err(|_| CloudError::Persistence)
    }
}
fn tcp(port: u16, source: &str) -> Rule {
    Rule {
        description: None,
        direction: Direction::In,
        protocol: Protocol::Tcp,
        port: Some(port.to_string()),
        source_ips: vec![source.into()],
        destination_ips: Vec::new(),
    }
}

#[test]
#[ignore = "Requires separately approved original normal worker, provider effects and exact cleanup"]
fn real_original_worker_management_retains_siblings_and_cleans() {
    run().unwrap();
}
fn run() -> Result<()> {
    let input = Input::load()?;
    let store = Store::lock_existing(&input.cloud)?;
    let state = store
        .load()?
        .ok_or(Error::Invalid("Original normal deployment is missing"))?;
    check_deployment(&input, &state)?;
    let worker = state
        .worker
        .as_ref()
        .ok_or(Error::Invalid("Original normal worker is missing"))?;
    let normal = NormalJournal::load(&input.cloud)?;
    let retained_key = normal
        .key
        .as_deref()
        .filter(|key| horizon_cloud::valid_public_key(key))
        .ok_or(Error::Invalid("Original provider public key is missing"))?;
    require(
        !normal.deleting
            && normal.released.is_none()
            && !normal.unused
            && matches!(&normal.volume, CreateState::Bound { worker_id } if worker_id == &input.volume.to_string()),
        "Original provider journal does not bind this worker",
    )?;
    let settings = Settings::load(&input.settings)?;
    let configured = settings
        .hetzner
        .as_ref()
        .ok_or(Error::Invalid("Original Hetzner settings missing"))?;
    input.credential.check(&configured.token_file)?;
    input.ssh_key.check(&settings.ssh_identity_file)?;
    let compute = Compute::new(&settings)?;
    let cancel = Cancellation::default();
    let server = compute
        .client
        .inspect_server(input.server, &cancel)?
        .ok_or(Error::Invalid("Original server absent"))?;
    server.verify(&input.allocation)?;
    let volume = compute
        .client
        .inspect_volume(input.volume, &cancel)?
        .ok_or(Error::Invalid("Original volume absent"))?;
    volume.verify(&input.allocation)?;
    let keys = compute.client.find_ssh_keys(&input.allocation, &cancel)?;
    require(
        keys.len() == 1
            && keys[0].id == input.key
            && keys[0].public_key.split_ascii_whitespace().take(2).collect::<Vec<_>>()
                == retained_key.split_ascii_whitespace().take(2).collect::<Vec<_>>(),
        "Original registered key changed",
    )?;
    keys[0].verify(&input.allocation)?;
    require(
        server.status == "running"
            && horizon_cloud::hetzner::cloud::holds(&server, &volume)
            && normal.location.as_deref() == Some(server.location.name.as_str())
            && server.public_net.firewalls.as_ref().is_some_and(Vec::is_empty),
        "Original server baseline changed",
    )?;
    let endpoint = server
        .ssh_address()
        .ok_or(Error::Invalid("Original SSH endpoint unavailable"))?;
    let connection = Connection::new(worker, &settings, &input.cloud)?;
    require(
        connection.host == endpoint.ip().to_string()
            && connection.port == endpoint.port()
            && !input.management_source.is_unspecified()
            && !input.management_source.is_loopback(),
        "Original pinned SSH endpoint or management source changed",
    )?;
    input.ssh(&connection, &cancel)?;
    let source = format!(
        "{}/{}",
        input.management_source,
        if input.management_source.is_ipv4() { 32 } else { 128 }
    );
    let management = management::ManagementRules::new(vec![tcp(connection.port, &source)])?;
    let scope = Scope::new(&input.test_operation, &input.allocation, input.server)?;
    let mut journal = Saved::new(&input)?;
    let mut manager = compute.client.prepare_exposure_management(scope, management, &cancel)?;
    require(manager.ssh_endpoint() == endpoint, "Manager SSH baseline changed")?;
    let work = exercise(&input, &connection, &mut manager, &mut journal, &cancel);
    // Keep the original client through every error; never reconstruct a controller.
    let cleanup = manager.cleanup_original(&mut journal, &Cancellation::default());
    cleanup?;
    require(
        manager.firewall_id().is_none() && !manager.uncertain(),
        "Original firewall cleanup is uncertain",
    )?;
    let retired = manager.replace_service("no-reopen", vec![tcp(20000, &source)], &mut journal, &cancel);
    require(
        matches!(retired, Err(CloudError::CreationUnresolved)),
        "Retired manager reopened",
    )?;
    input.ssh(&connection, &cancel)?;
    input.check()?;
    drop(store);
    work
}
fn check_deployment(input: &Input, state: &Deployment) -> Result<()> {
    let spec = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Original normal spec is missing"))?;
    let worker = state
        .worker
        .as_ref()
        .ok_or(Error::Invalid("Original normal worker is missing"))?;
    require(
        state.worker_ready()
            && state.cloud_id == input.cloud_id
            && spec.operation_id == input.allocation
            && worker.id == input.server.to_string()
            && state.profile.provider == horizon_cloud::hetzner::PROVIDER
            && state.profile == spec.profile
            && worker.image_name == spec.image_digest
            && state.service_admission.is_none()
            && state.exposure.is_none()
            && state.image_replacement.is_none()
            && !state.profile.bootstrap.exposure_ingress
            && worker.docker_host.is_none()
            && worker
                .port_mappings
                .as_ref()
                .is_some_and(|ports| ports.len() == 1 && ports.get("22") == Some(&22))
            && matches!(&state.operation, CreateState::Bound { worker_id } if worker_id == &worker.id),
        "Original normal worker is not an idle SSH-only Hetzner allocation",
    )?;
    require(
        Selection::load(&input.cloud)
            .map_err(|_| Error::Invalid("Tailnet selection unreadable"))?
            .tailnet
            .is_none()
            && state.tailnet_enrollment.is_empty(),
        "Qualification requires Tailnet=None",
    )
}
fn exercise(
    input: &Input,
    connection: &Connection,
    manager: &mut management::Manager,
    journal: &mut Saved,
    cancel: &Cancellation,
) -> Result<()> {
    let a = vec![tcp(20000, "192.0.2.0/24")];
    let b = vec![tcp(20001, "203.0.113.0/24")];
    input.check()?;
    manager.replace_service("service-a", a, journal, cancel)?;
    manager.verify_current(cancel)?;
    input.ssh(connection, cancel)?;
    input.check()?;
    manager.replace_service("service-b", b.clone(), journal, cancel)?;
    manager.verify_current(cancel)?;
    input.ssh(connection, cancel)?;
    manager.close_service_retaining_firewall("service-a", journal, cancel)?;
    manager.verify_current(cancel)?;
    input.ssh(connection, cancel)?;
    manager.close_service_retaining_firewall("service-b", journal, cancel)?;
    manager.verify_current(cancel)?;
    input.ssh(connection, cancel)?;
    let original = manager.firewall_id();
    manager.replace_service("service-b", b, journal, cancel)?;
    require(
        original.is_some() && manager.firewall_id() == original,
        "Live epoch replaced its firewall",
    )?;
    manager.verify_current(cancel)?;
    input.ssh(connection, cancel)
}
