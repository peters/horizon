//! Two computers and one server: the machine keys and the credential values of
//! this slice, with a stand-in `ssh` in place of a connection.
//!
//! The stand-in is the one of the supervisor tests: a `sh` program that runs a
//! small Python relay against the socket of the temporary server, so no test opens
//! a connection and no real `ssh` runs. The OS credential store is the in-memory
//! store that the workbench tests use, behind the same sealed trait.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use horizon_browser::remote::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteBrowserConfig, RemoteProviderProfile, RemoteSessionLimits,
};
use horizon_core::machines::{MachineBindings, MachineRegistry, Machines};
use horizon_core::named_secrets::{NamedSecretVault, SecretName, published_names};
use horizon_core::remote_browser_credential::{
    CredentialLocator, CredentialStores, RemoteCredentialStore, SessionCredentialStore, SharedCredentialStore,
    resolve_authorization,
};
use horizon_core::secret::{CredentialVault, SecretId, SecretTarget, SecretVault};
use horizon_core::secret_store::SecretStore;
use horizon_core::{Machine, MachineKeyFiles, MachineKeys};
use horizon_sync::{
    LinkBounds, LinkState, SocketServer, Store, Supervisor, SupervisorConfig, SyncHandle, SyncedMachineStore,
    run_server_on_unix_socket,
};

/// The time a test gives a change to travel through a link and a store.
const PATIENCE: Duration = Duration::from_secs(30);

/// The marker of the private text of a key in every test of this file.
const MARKER: &str = "MARKER-7f3a-private-text";

/// The marker of the value of a named secret in this file.
const NAMED_MARKER: &str = "MARKER-58c2-named-secret-value";

/// The relay of one stand-in `ssh`: the bytes of its standard input and output go
/// to the socket of the server in both directions.
const RELAY: &str = "\
import os
import socket
import sys
import threading

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])


def outward():
    try:
        while True:
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            sock.sendall(data)
    except Exception:
        pass
    try:
        sock.shutdown(socket.SHUT_WR)
    except Exception:
        pass


threading.Thread(target=outward, daemon=True).start()
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except Exception:
    pass
";

/// A stand-in `ssh` for one computer.
fn stand_in(directory: &Path, socket: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("relay.py");
    fs::write(&relay, RELAY).expect("the relay of the stand-in");
    let program = directory.join("ssh");
    let script = format!(
        "#!/bin/sh\nexec /usr/bin/env python3 '{}' '{}'\n",
        relay.display(),
        socket.display()
    );
    fs::write(&program, script).expect("the stand-in ssh");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("an executable stand-in");
    program.to_string_lossy().into_owned()
}

/// The provider of the credential tests: one endpoint whose two references are
/// both bound to the OS credential store.
fn provider(slot: &str) -> RemoteProviderProfile {
    let mut credential_bindings = BTreeMap::new();
    for (reference, name) in [("user", "username"), ("key", "password")] {
        credential_bindings.insert(
            CredentialReference::from(reference),
            CredentialBinding {
                store: CredentialStoreKind::File,
                slot: Some(format!("remote-browser/{slot}/{name}")),
            },
        );
    }
    RemoteProviderProfile {
        adapter: RemoteAdapterKind::Webdriver,
        endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("the endpoint"),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings,
        limits: RemoteSessionLimits::default(),
    }
}

fn remote() -> RemoteBrowserConfig {
    let mut remote = RemoteBrowserConfig::default();
    remote.providers.insert("grid".to_owned(), provider("a"));
    remote
}

/// The vault of one computer whose secrets file holds no value, for a test that
/// writes a machine key and no credential.
fn keys_only(keys: MachineKeys) -> SecretVault {
    let credentials = Arc::new(CredentialVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    credentials.register(&remote());
    let named = Arc::new(NamedSecretVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    SecretVault::new(keys, credentials, named)
}

/// The bytes of one machine key, as the server holds them.
fn key_files(private_key: &str) -> Vec<u8> {
    serde_json::to_vec(&MachineKeyFiles {
        private_key: private_key.to_owned(),
        public_key: format!(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{} horizon-machine",
            private_key.len()
        ),
        known_hosts: "203.0.113.10 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHOST".to_owned(),
    })
    .expect("a machine key")
}

/// The private text of one machine key body.
fn private_of(body: &[u8]) -> String {
    serde_json::from_slice::<MachineKeyFiles>(body)
        .expect("a machine key body")
        .private_key
}

fn machine(name: &str, host: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: host.to_owned(),
        added_at_millis: 1,
        ..Machine::default()
    }
}

/// The lines of the log of every test of this file, so a test can prove that the
/// private text of a key reached no line.
fn traced() -> Arc<Mutex<Vec<u8>>> {
    static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    Arc::clone(BUFFER.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buffer);
        let installed = tracing_subscriber::fmt()
            .with_writer(move || Sink(Arc::clone(&sink)))
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .try_init();
        assert!(installed.is_ok(), "the log of the test is captured once");
        buffer
    }))
}

/// One sink of the captured log.
struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("the log").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One server of a temporary home, which a test can stop and start again over the
/// same store.
struct Server {
    home: PathBuf,
    socket: PathBuf,
    store: Store,
    running: Option<SocketServer>,
}

impl Server {
    fn start(home: &Path) -> Self {
        fs::create_dir_all(home).expect("the server home");
        let socket = home.join("server.sock");
        let store = Store::open(&home.join("server.sqlite")).expect("the store");
        let running = Some(
            run_server_on_unix_socket(&socket, store.clone(), &horizon_sync::ServerInfo::default())
                .expect("the socket"),
        );
        Self {
            home: home.to_path_buf(),
            socket,
            store,
            running,
        }
    }

    fn stop(&mut self) {
        if let Some(server) = self.running.take() {
            server.stop();
        }
    }

    fn restart(&mut self) {
        self.running = Some(
            run_server_on_unix_socket(&self.socket, self.store.clone(), &horizon_sync::ServerInfo::default())
                .expect("the socket"),
        );
    }

    /// The secret rows of the store, by id, as a pull reports them.
    fn secrets(&self) -> BTreeMap<String, String> {
        self.store
            .pull(0, "")
            .expect("the rows of the server")
            .rows
            .into_iter()
            .filter(|row| !row.deleted && row.kind == horizon_sync::KIND_SECRET)
            .filter_map(|row| row.body.as_str().map(|body| (row.id, body.to_owned())))
            .collect()
    }

    fn bindings(&self) -> BTreeMap<String, String> {
        self.store
            .pull(0, "")
            .expect("the rows of the server")
            .rows
            .into_iter()
            .filter(|row| !row.deleted && row.kind == horizon_sync::KIND_BINDING)
            .filter_map(|row| {
                row.body
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .map(|key| (row.id, key.to_owned()))
            })
            .collect()
    }

    /// The text of the log table of the store file, for a test that proves that no
    /// secret body reached it.
    fn log_text(&self) -> String {
        let connection = rusqlite::Connection::open(self.home.join("server.sqlite")).expect("the file");
        let mut statement = connection
            .prepare("SELECT kind, object_id, result_json FROM log")
            .expect("the log");
        let rows: Vec<(String, String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("the rows")
            .collect::<Result<_, _>>()
            .expect("the rows");
        rows.into_iter()
            .map(|(kind, id, result)| format!("{kind} {id} {result}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// One computer of the test: its machine directory, its keys, its credential
/// store, and the supervisor that follows the server.
struct Computer {
    machines_dir: PathBuf,
    keys: MachineKeys,
    credentials: Arc<CredentialVault>,
    values: Arc<Mutex<Arc<SecretStore>>>,
    /// The credential store of the named secrets of this computer.
    named_values: Arc<Mutex<Arc<SecretStore>>>,
    named: Arc<NamedSecretVault>,
    supervisor: Option<Supervisor>,
    server: PathBuf,
}

impl Computer {
    fn start(home: &Path, socket: &Path) -> Self {
        fs::create_dir_all(home).expect("the machine directory");
        let machines_dir = home.to_path_buf();
        let keys = MachineKeys::new(home.join("machine-keys"));
        // The one secrets file of this computer, not a store in memory: a value that
        // arrives from the server is a row of the file that the code of this
        // computer reads.
        let secrets_file = home.join(horizon_core::secret_store::SECRETS_FILE_NAME);
        let values = Arc::new(Mutex::new(
            SecretStore::open(secrets_file.clone()).expect("the secrets file"),
        ));
        let credentials = Arc::new(CredentialVault::new(Box::new(SharedCredentialStore::new(Arc::clone(
            &values,
        )))));
        credentials.register(&remote());
        let named_values = Arc::new(Mutex::new(SecretStore::open(secrets_file).expect("the secrets file")));
        let named = Arc::new(NamedSecretVault::new(Box::new(SharedCredentialStore::new(Arc::clone(
            &named_values,
        )))));
        let vault = SecretVault::new(keys.clone(), Arc::clone(&credentials), Arc::clone(&named));
        let mut config = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            &machines_dir,
            Arc::new(vault),
        )
        .expect("the settings of the link");
        config.program = stand_in(&home.join("stand-in"), socket);
        config.bounds = LinkBounds::STANDARD;
        let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
        Self {
            machines_dir,
            keys,
            credentials,
            values,
            named_values,
            named,
            supervisor: Some(supervisor),
            server: socket.to_path_buf(),
        }
    }

    fn start_again(&mut self) {
        let vault = SecretVault::new(
            self.keys.clone(),
            Arc::clone(&self.credentials),
            Arc::clone(&self.named),
        );
        let mut config = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            &self.machines_dir,
            Arc::new(vault),
        )
        .expect("the settings of the link");
        config.program = stand_in(&self.machines_dir.join("stand-in"), &self.server);
        self.supervisor = Some(Supervisor::start(config, || {}).expect("the supervisor"));
    }

    fn handle(&self) -> &SyncHandle {
        self.supervisor.as_ref().expect("a running supervisor").handle()
    }

    fn stop(&mut self) {
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.stop();
        }
    }

    fn state(&self) -> LinkState {
        self.handle().state()
    }

    /// Makes a machine key of this computer as `machine_key` does, and queues its
    /// value for the server.
    fn make_key(&self, name: &str) {
        self.keys.ensure(name, 32).expect("a machine key");
        self.share_key(name);
    }

    /// Queues the value of a key that is here, as the tool does after it made one.
    fn share_key(&self, name: &str) {
        let id = SecretId::machine_key(name).expect("an id");
        self.handle().queue_secret(&id).expect("the queue");
    }

    fn delete_key(&self, name: &str) {
        self.keys.delete(name).expect("the key is deleted");
        let id = SecretId::machine_key(name).expect("an id");
        self.handle().queue_secret_remove(&id).expect("the queue");
    }

    /// Sets a credential value of this computer as the settings do, and queues it.
    fn set_credential(&self, reference: &str, value: &[u8]) {
        self.credentials.write(reference, value).expect("the value");
        let id = SecretId::credential(reference).expect("an id");
        self.handle().queue_secret(&id).expect("the queue");
    }

    /// The value under `reference`, as the link writes it into the OS store.
    fn credential(&self, reference: &str) -> Option<Vec<u8>> {
        let profile = provider("a");
        let reference = CredentialReference::from(reference);
        let locator = CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference]);
        let store = self.values.lock().expect("the store");
        horizon_core::secret::read_secret_value(&*store as &dyn RemoteCredentialStore, &locator)
            .ok()
            .map(|value| value.to_vec())
    }

    /// The authorization header that the existing read path of a provider builds
    /// from the values of this computer.
    fn header(&self) -> Option<String> {
        let profile = provider("a");
        let session = SessionCredentialStore::new();
        let store = self.values.lock().expect("the store");
        let stores = CredentialStores {
            session: &session,
            secrets_file: Some(&*store as &dyn RemoteCredentialStore),
            environment: None,
        };
        resolve_authorization(&profile, &stores)
            .ok()
            .flatten()
            .map(|authorization| authorization.header_value().to_owned())
    }

    /// The machine list of this computer, read through the store of its link.
    fn machines(&self) -> Machines {
        let registry = MachineRegistry::new(self.machines_dir.clone());
        let store = Arc::new(SyncedMachineStore::new(self.handle().clone()));
        Machines::from_store(&registry, store).expect("the machine list")
    }

    /// Binds a key to a machine as a click and `machine_set` do, and queues the
    /// binding.
    fn bind(&self, machine_name: &str, key: &str) {
        let registry = MachineRegistry::new(self.machines_dir.clone());
        let store = Arc::new(SyncedMachineStore::new(self.handle().clone()));
        let mut list = Machines::from_store(&registry, store).expect("the machine list");
        let mut bindings = MachineBindings::in_directory(&self.machines_dir);
        bindings.set(machine_name, Some(key));
        bindings.save().expect("the bindings file");
        list = list.with_key(machine_name, key);
        list.save().expect("the list and its binding");
    }

    /// Adds a machine as the sidebar does.
    fn add(&self, name: &str, host: &str) {
        let list = self.machines();
        let (list, _) = list.with_record(machine(name, host), None).expect("a machine");
        list.save().expect("the machine list");
    }

    fn files(&self, name: &str) -> Option<MachineKeyFiles> {
        self.keys.files(name).expect("the files")
    }

    /// Stores a named secret as the Settings window does, and queues it.
    fn set_named(&self, name: &str, value: &[u8]) {
        let name = SecretName::parse(name).expect("a name of a named secret");
        self.named.put(&name, value).expect("the value");
        self.handle()
            .queue_secret(&SecretId::named(name.as_str()).expect("an id"))
            .expect("the queue");
    }

    /// The value of a named secret as this computer's credential store holds it.
    fn named_in_store(&self, name: &str) -> Option<Vec<u8>> {
        let name = SecretName::parse(name).ok()?;
        let store = self.named_values.lock().expect("the store");
        horizon_core::secret::read_secret_value(
            &*store as &dyn RemoteCredentialStore,
            &horizon_core::named_secrets::locator(&name),
        )
        .ok()
        .map(|value| value.to_vec())
    }

    /// The value of a named secret of this computer, as an agent panel reads it.
    fn named_value(&self, name: &str) -> Option<Vec<u8>> {
        let name = SecretName::parse(name).expect("a name of a named secret");
        self.named.value(&name).ok().flatten().map(|value| value.to_vec())
    }

    /// Deletes a named secret as the Settings window does, and queues the removal.
    fn delete_named(&self, name: &str) {
        let name = SecretName::parse(name).expect("a name of a named secret");
        self.named.remove(&name).expect("the removal");
        self.handle()
            .queue_secret_remove(&SecretId::named(name.as_str()).expect("an id"))
            .expect("the queue");
    }
}

/// Waits for an answer that a link or a store needs time to give.
fn eventually<T>(what: &str, mut answer: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = answer() {
            return value;
        }
        assert!(Instant::now() < deadline, "the test waited for {what} in vain");
        thread::sleep(Duration::from_millis(20));
    }
}

fn in_step(computer: &Computer) {
    eventually("the link of a computer", || {
        matches!(computer.state(), LinkState::InStep { .. }).then_some(())
    });
}

fn settled(computer: &Computer) {
    eventually("the queue of a computer", || {
        computer.handle().pending().is_empty().then_some(())
    });
}

/// The name of the private file of a conflict copy of `name`, if there is one.
fn conflict_copy(directory: &Path, name: &str) -> Option<String> {
    let prefix = format!("{name}.conflict-");
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|candidate| {
            let candidate = Path::new(candidate);
            let text = candidate.to_string_lossy();
            text.starts_with(&prefix)
                && candidate.extension().and_then(std::ffi::OsStr::to_str) != Some("pub")
                && candidate.extension().and_then(std::ffi::OsStr::to_str) != Some("known_hosts")
                && candidate.extension().and_then(std::ffi::OsStr::to_str) != Some("tmp")
        })
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("the file").permissions().mode() & 0o777
}

#[test]
fn a_machine_key_of_one_computer_arrives_at_the_other_and_its_argv_uses_it() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);

    a.add("build-1", "203.0.113.10");
    eventually("the machine at both computers", || {
        (b.machines().find("build-1").is_some()).then_some(())
    });
    a.make_key("build-1");
    a.bind("build-1", "build-1");
    // The queue holds the id of the key, not its text.
    let queue = fs::read_to_string(b.machines_dir.join("server-queue.json")).unwrap_or_default();
    assert!(!queue.contains("PRIVATE KEY"), "the queue of a computer holds no key");

    eventually("the key at the second computer", || {
        ["build-1", "build-1.pub", "build-1.known_hosts"]
            .iter()
            .all(|name| b.machines_dir.join("machine-keys").join(name).is_file())
            .then_some(())
    });
    let private_key = b.keys.require("build-1").expect("the key of this computer");
    let directory = b.machines_dir.join("machine-keys");
    assert_eq!(mode(&directory), 0o700, "the key folder is private");
    for name in ["build-1", "build-1.pub", "build-1.known_hosts"] {
        assert_eq!(mode(&directory.join(name)), 0o600, "{name} is private");
    }
    let theirs = a.files("build-1").expect("the key of the first computer");
    let ours = b.files("build-1").expect("the key of the second computer");
    assert_eq!(ours, theirs, "the two computers hold one pair");
    assert!(
        private_key.private_key().is_file(),
        "the key path that the argv builder names is there"
    );
    let argv = private_key.ssh_option_args();
    assert!(
        argv.iter()
            .any(|argument| argument == &private_key.private_key().display().to_string()),
        "the argv of the second computer names the key that arrived: {argv:?}"
    );
    assert!(
        argv.iter().any(|argument| argument.ends_with("build-1.known_hosts")),
        "the known hosts of that key are used: {argv:?}"
    );

    // The key binding follows the value, so a click on the machine uses the key.
    eventually("the binding of the machine", || {
        (b.machines().key("build-1") == Some("build-1")).then_some(())
    });
    assert_eq!(server.bindings().get("build-1"), Some(&"build-1".to_string()));
    assert!(
        b.handle().has_machine_key("build-1"),
        "the machine section knows that the key is here"
    );
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_key_deleted_on_one_computer_is_gone_on_the_other() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.make_key("build-1");
    eventually("the key at the second computer", || {
        b.keys.files("build-1").expect("the files").map(|_| ())
    });

    // The second computer deletes the key, as `delete_machine_key` does.
    b.delete_key("build-1");
    eventually("the removal at the first computer", || {
        a.keys.files("build-1").expect("the files").is_none().then_some(())
    });
    assert!(
        a.files("build-1").is_none() && b.files("build-1").is_none(),
        "no computer keeps the pair"
    );
    assert!(server.secrets().is_empty(), "the server keeps no value");
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_credential_value_set_on_one_computer_reads_on_the_other() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    assert!(b.credential("key").is_none(), "the second computer holds no value");

    a.set_credential("key", b"s3cret-value");
    a.set_credential("user", b"alice");
    // The existing read path of the second computer finds the value, because it is
    // written at the address that its own settings bind to the reference.
    eventually("the value at the second computer", || b.credential("key"));
    eventually("the user of the second computer", || b.credential("user"));
    assert_eq!(a.credential("key"), b.credential("key"), "one value on both computers");
    // Two computers on one machine are two private homes and two files, so a value
    // of one is a row of the file of the other only after the link carried it.
    let file_a = home
        .path()
        .join("a")
        .join(horizon_core::secret_store::SECRETS_FILE_NAME);
    let file_b = home
        .path()
        .join("b")
        .join(horizon_core::secret_store::SECRETS_FILE_NAME);
    assert!(file_a.is_file() && file_b.is_file(), "each computer holds its own file");
    assert_ne!(file_a, file_b);
    assert_eq!(
        mode(&file_a),
        0o600,
        "the secrets file of the first computer is private"
    );
    assert_eq!(
        mode(&file_b),
        0o600,
        "the secrets file of the second computer is private"
    );
    let header = b.header().expect("the header of the provider");
    assert!(header.starts_with("Basic "), "the provider builds its header: {header}");
    assert_eq!(Some(header), a.header(), "the two computers build one header");
    assert!(
        server.secrets().contains_key("credential/key"),
        "the server holds the value under the reference"
    );

    // A delete of the value reaches the other computer too.
    a.credentials.remove("key").expect("the value is removed");
    a.handle()
        .queue_secret_remove(&SecretId::credential("key").expect("an id"))
        .expect("the queue");
    eventually("the removal at the second computer", || {
        b.credential("key").is_none().then_some(())
    });
    settled(&a);
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_secret_that_waits_for_an_away_server_settles_when_it_returns() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);

    server.stop();
    eventually("the link that learns that the server is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });
    a.make_key("build-1");
    eventually("the key that waits for the server", || {
        a.handle().pending().contains("machine-key/build-1").then_some(())
    });

    server.restart();
    eventually("the key at the second computer", || {
        b.keys.files("build-1").expect("the files").map(|_| ())
    });
    assert!(a.handle().pending().is_empty(), "the queue is settled");
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn the_loser_of_the_version_rule_keeps_its_key_as_a_conflict_copy() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.make_key("build-1");
    eventually("the key at both computers", || {
        (b.keys.files("build-1").expect("the files").is_some()
            && a.handle().secret_version("machine-key/build-1").is_some())
        .then_some(())
    });
    eventually("the version at the second computer", || {
        b.handle().secret_version("machine-key/build-1")
    });

    // Both computers change the key while the server is away, on the version that
    // both saw.
    server.stop();
    eventually("both links away", || {
        (matches!(a.state(), LinkState::Away { .. }) && matches!(b.state(), LinkState::Away { .. })).then_some(())
    });
    let theirs = SecretId::machine_key("build-1").expect("an id");
    let value_a = key_files(&format!("PRIVATE-A-{MARKER}"));
    let value_b = key_files(&format!("PRIVATE-B-{MARKER}"));
    a.handle().queue_secret(&theirs).expect("the queue of the first");
    b.handle().queue_secret(&theirs).expect("the queue of the second");
    // The values are written where a local change writes them.
    let vault_a = keys_only(a.keys.clone());
    let vault_b = keys_only(b.keys.clone());
    vault_a.write(&theirs, &value_a).expect("the first value");
    vault_b.write(&theirs, &value_b).expect("the second value");

    // Only the first computer reaches the server, so the first write wins.
    b.stop();
    server.restart();
    eventually("the first value at the server", || {
        (server.secrets().get("machine-key/build-1") == Some(&String::from_utf8(value_a.clone()).expect("text")))
            .then_some(())
    });

    // The second computer answers its own queued intent again, and loses.
    b.start_again();
    eventually("the value of the server at the second computer", || {
        (b.files("build-1").map(|files| files.private_key) == Some(private_of(&value_a))).then_some(())
    });
    let conflict = eventually("the copy of the lost key", || {
        conflict_copy(&b.machines_dir.join("machine-keys"), "build-1")
    });
    let kept = fs::read_to_string(b.machines_dir.join("machine-keys").join(&conflict)).expect("the copy");
    assert_eq!(kept, private_of(&value_b), "the copy is the lost value");
    assert!(
        b.keys.list().expect("the keys").iter().all(|key| key.name == "build-1"),
        "a conflict copy is not a key that an agent may use"
    );
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_key_that_the_server_holds_the_same_way_is_no_conflict_copy_and_no_notice() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.make_key("build-1");
    eventually("the key at both computers", || {
        (b.keys.files("build-1").expect("the files").is_some()
            && a.handle().secret_version("machine-key/build-1").is_some())
        .then_some(())
    });
    let theirs = a.files("build-1").expect("the key of the server");
    assert_eq!(
        b.files("build-1").map(|files| files.private_key),
        Some(theirs.private_key.clone()),
        "both computers hold one key"
    );

    // The second computer pushes its own key again without the version that the
    // server holds, as a computer does after the server lost its key file: the
    // server answers `already_answered`, and its value is the value of this
    // computer.
    b.stop();
    let device = fs::read_to_string(b.machines_dir.join("server-device")).expect("the device name");
    let mut queue = horizon_sync::Client::open(b.machines_dir.join("server-queue.json"), device.trim())
        .expect("the queue of the second computer");
    let id = SecretId::machine_key("build-1").expect("an id");
    queue.forget_secrets().expect("the versions are forgotten");
    queue.enqueue_secret(id.as_str()).expect("the key is queued again");
    drop(queue);
    b.start_again();
    settled(&b);
    eventually("the version of the value of the server", || {
        b.handle().secret_version(id.as_str())
    });
    // The frame that decides about a refused value follows the frame that brings
    // it, so the test gives the link that one turn.
    thread::sleep(Duration::from_millis(300));

    assert_eq!(
        conflict_copy(&b.machines_dir.join("machine-keys"), "build-1"),
        None,
        "a key that the server holds the same way keeps no copy"
    );
    assert!(
        b.handle().take_secret_notices().is_empty(),
        "the person reads no notice about a key that did not change"
    );
    assert_eq!(
        b.files("build-1").map(|files| files.private_key),
        Some(theirs.private_key.clone()),
        "the key of this computer still opens the machine"
    );
    a.stop();
    server.stop();
}

#[test]
fn a_server_that_lost_its_key_keeps_running_and_a_computer_sends_its_secrets_again() {
    let home = tempfile::tempdir().expect("a directory");
    let server_home = home.path().join("server");
    let mut server = Server::start(&server_home);
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);
    a.make_key("build-1");
    eventually("the value at the server", || {
        server.secrets().contains_key("machine-key/build-1").then_some(())
    });

    // The key file of the server goes away, as a restore of the database alone
    // leaves it.
    a.stop();
    server.stop();
    let stopped = server.store.clone();
    stopped.close();
    drop(server);
    fs::remove_file(server_home.join("secret.key")).expect("the key is lost");
    let mut server = Server::start(&server_home);
    assert!(server.store.secrets_lost(), "the server reports the loss");
    assert!(
        server.secrets().is_empty(),
        "a value that this key cannot open is no row of a pull"
    );

    // The computer that still holds the value sends it again, without a person.
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    eventually("the value at the server again", || {
        server.secrets().contains_key("machine-key/build-1").then_some(())
    });
    assert!(!server.store.secrets_lost(), "the value that came again opens");
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    eventually("the value at a new computer", || {
        b.keys.files("build-1").expect("the files").map(|_| ())
    });
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn the_first_link_of_a_computer_that_already_has_keys_keeps_the_value_of_the_server() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);
    a.make_key("build-1");
    eventually("the key at the server", || {
        server.secrets().contains_key("machine-key/build-1").then_some(())
    });

    // A computer that kept keys of its own before it followed the server.
    let b_home = home.path().join("b");
    fs::create_dir_all(b_home.join("machine-keys")).expect("the key folder");
    fs::set_permissions(b_home.join("machine-keys"), fs::Permissions::from_mode(0o700)).expect("the mode");
    let b_keys = MachineKeys::new(b_home.join("machine-keys"));
    let local = keys_only(b_keys.clone());
    let theirs = SecretId::machine_key("build-1").expect("an id");
    let value_b = key_files(&format!("PRIVATE-B-{MARKER}"));
    local.write(&theirs, &value_b).expect("the local key");
    let own = SecretId::machine_key("own").expect("an id");
    let value_own = key_files(&format!("PRIVATE-OWN-{MARKER}"));
    local.write(&own, &value_own).expect("the other local key");

    let mut b = Computer::start(&b_home, &server.socket);
    in_step(&b);
    // The union: the key of this computer that the server does not have is sent.
    eventually("the own key at the server", || {
        server.secrets().contains_key("machine-key/own").then_some(())
    });
    // A name that both have keeps the value of the server, and the local value is
    // kept beside it.
    let theirs_files = a.files("build-1").expect("the key of the server");
    eventually("the value of the server at the second computer", || {
        (b.files("build-1").map(|files| files.private_key) == Some(theirs_files.private_key.clone())).then_some(())
    });
    let conflict = eventually("the copy of the local key", || {
        conflict_copy(&b_home.join("machine-keys"), "build-1")
    });
    assert_eq!(
        fs::read_to_string(b_home.join("machine-keys").join(conflict)).expect("the copy"),
        private_of(&value_b)
    );
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn the_body_of_a_secret_reaches_no_file_of_a_computer_and_no_line_of_the_log() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.add("build-1", "203.0.113.10");
    // The key holds a marker, so a test can look for its private text everywhere.
    let id = SecretId::machine_key("build-1").expect("an id");
    let vault = keys_only(a.keys.clone());
    vault
        .write(
            &id,
            &key_files(&format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{MARKER}\n")),
        )
        .expect("the local key");
    a.handle().queue_secret(&id).expect("the queue");
    eventually("the key at the second computer", || {
        b.keys.files("build-1").expect("the files").map(|_| ())
    });
    settled(&b);
    let private = a.files("build-1").expect("the key").private_key;
    assert!(private.contains(MARKER), "the private text is where a key is read");
    for path in [
        a.machines_dir.join("server-queue.json"),
        a.machines_dir.join("server-mirror.json"),
        b.machines_dir.join("server-queue.json"),
        b.machines_dir.join("server-mirror.json"),
    ] {
        let text = fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains(MARKER), "{} holds no secret body", path.display());
        assert!(!text.contains("PRIVATE KEY"), "{} holds no key text", path.display());
    }
    let log = server.log_text();
    assert!(log.contains("machine-key/build-1"), "the log names the secret");
    assert!(!log.contains(MARKER), "the log holds no secret body");
    let trace = String::from_utf8_lossy(&traced().lock().expect("the log")).into_owned();
    assert!(!trace.contains(MARKER), "no line of the log holds a secret body");
    assert!(
        !trace.contains("PRIVATE KEY"),
        "no line of the log holds the text of a key"
    );
    a.stop();
    b.stop();
    server.stop();
}

/// The queue of a computer holds the id of a secret and not its value, and the
/// debug of an intent redacts the body.
#[test]
fn a_queued_secret_is_one_id_and_its_debug_redacts_the_body() {
    let temp = tempfile::tempdir().expect("a directory");
    let device = "mbp-7c1";
    let mut client = horizon_sync::Client::open(temp.path().join("queue.json"), device).expect("a queue");
    let id = SecretId::credential("key").expect("an id");
    client.enqueue_secret(id.as_str()).expect("a secret");
    let text = fs::read_to_string(temp.path().join("queue.json")).expect("the queue file");
    assert!(text.contains("credential/key"), "{text}");
    assert!(text.contains("\"body\": null"), "the queue holds no body: {text}");
    let chunk = client.next_push().expect("one push");
    let op = &chunk[0].op;
    assert!(op.secret_body().is_none());
    assert_eq!(format!("{op:?}"), "SecretPut { id: \"credential/key\", body: None }");
    let filled = horizon_sync::Op::SecretPut {
        id: id.as_str().to_owned(),
        body: Some(horizon_sync::SecretBody::new(MARKER.to_owned())),
    };
    let debug = format!("{filled:?}");
    assert!(debug.contains("redacted") && !debug.contains(MARKER), "{debug}");
    assert!(!text.contains(MARKER));
}

#[test]
fn a_named_secret_of_one_computer_is_the_environment_of_an_agent_panel_on_the_other() {
    // The published environment is one map of the test process, so this test is the
    // only one of this file that writes it.
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.named.set_names(["HETZNER_TOKEN".to_owned()]);
    b.named.set_names(["HETZNER_TOKEN".to_owned()]);
    assert!(
        b.named_value("HETZNER_TOKEN").is_none(),
        "the second computer holds no value"
    );

    a.set_named("HETZNER_TOKEN", NAMED_MARKER.as_bytes());
    // The value of the second computer arrives, and its agent panels read it: the
    // vault of the second computer publishes it for the next panel.
    eventually("the named secret at the second computer", || {
        b.named_value("HETZNER_TOKEN")
    });
    assert_eq!(
        b.named_value("HETZNER_TOKEN").as_deref(),
        Some(NAMED_MARKER.as_bytes()),
        "one value on both computers"
    );
    assert_eq!(
        b.named_in_store("HETZNER_TOKEN").as_deref(),
        Some(NAMED_MARKER.as_bytes()),
        "the value is in the credential store of the second computer"
    );
    assert!(
        b.named
            .refresh()
            .expect("the read of the second computer")
            .iter()
            .any(|name| name.as_str() == "HETZNER_TOKEN"),
        "the names of the second computer hold the value"
    );
    assert!(
        published_names().iter().any(|name| name.as_str() == "HETZNER_TOKEN"),
        "the next agent panel of this process reads the value"
    );
    assert!(
        server.secrets().contains_key("named/HETZNER_TOKEN"),
        "the server holds the value under the name"
    );
    assert!(
        !String::from_utf8_lossy(&fs::read(server.home.join("server.sqlite")).expect("the store file"))
            .contains(NAMED_MARKER),
        "the database of the server holds no value in clear"
    );
    assert!(
        !server.log_text().contains(NAMED_MARKER),
        "the log of the server holds no value"
    );

    // A delete on the second computer removes the value on the first one too.
    b.delete_named("HETZNER_TOKEN");
    eventually("the removal at the first computer", || {
        a.named_value("HETZNER_TOKEN").is_none().then_some(())
    });
    eventually("the removal at the server", || {
        (!server.secrets().contains_key("named/HETZNER_TOKEN")).then_some(())
    });
    settled(&a);
    settled(&b);
    a.stop();
    b.stop();
    server.stop();
}
