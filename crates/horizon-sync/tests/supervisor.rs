//! Two computers and one server, with a stand-in `ssh` in place of a connection.
//!
//! The stand-in is one `sh` program, as the browser forward tests use: it writes
//! its own arguments, so a test reads the bounded flags of the call, and then runs
//! a small Python relay against the socket of the temporary server. Python is the
//! interpreter that the device fixture of this repository already requires on
//! Linux and macOS, so no test opens a connection and no system tool is assumed.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use horizon_core::machines::MachineStore;
use horizon_core::{AppearanceTheme, Config, Machine};
use horizon_sync::{
    Client, IntentResult, LinkBounds, LinkRefusal, LinkState, MAX_INTENTS_PER_PUSH, Outcome, PushReply, ServerReply,
    ServerRequest, SocketServer, Store, Supervisor, SupervisorConfig, SyncedMachineStore, run_server_on_unix_socket,
};

/// The time a test gives a change to travel through a link and a store.
///
/// The bound is a budget of the test and not a rule of the product: a change
/// travels through one push, one pull and one store on each computer, and a
/// link that was lost waits 3, 6, 12 and 24 seconds and then 30 seconds before
/// its next attempt. A computer that runs this suite beside other work took
/// more than 30 seconds for one of these waits.
const PATIENCE: Duration = Duration::from_secs(180);

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

/// A computer of a test keeps machine keys and no credential value.
/// The machine keys of a computer whose secrets file holds no value: one store in
/// memory for the credential values and one for the named secrets.
fn keys_only(keys: horizon_core::MachineKeys) -> std::sync::Arc<dyn horizon_core::SecretTarget> {
    use horizon_core::named_secrets::NamedSecretVault;
    use horizon_core::secret::{CredentialVault, SecretVault};
    use horizon_core::secret_store::SecretStore;

    let credentials = std::sync::Arc::new(CredentialVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
    let named = std::sync::Arc::new(NamedSecretVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    std::sync::Arc::new(SecretVault::new(keys, credentials, named))
}

fn machine(name: &str, host: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: host.to_owned(),
        added_at_millis: 1,
        ..Machine::default()
    }
}

/// A stand-in `ssh` that records its arguments and then runs the link relay.
fn stand_in(directory: &Path, socket: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("relay.py");
    fs::write(&relay, RELAY).expect("the relay of the stand-in");
    let arguments = directory.join("ssh-arguments.txt");
    let script = format!(
        "printf '%s\\n' \"$@\" > '{}'\nexec /usr/bin/env python3 '{}' '{}'",
        arguments.display(),
        relay.display(),
        socket.display()
    );
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// The relay of an `ssh` whose server stopped answering after its hello: it
/// answers the hello and then keeps both pipes open without a frame. It adds one
/// line to its `starts` file for every start, so a test sees that a lost link is
/// ended and started again.
const SILENT_RELAY: &str = "\
import json
import struct
import sys
import time

with open(sys.argv[1], 'a') as starts:
    starts.write('start\\n')


def read_frame():
    head = sys.stdin.buffer.read(4)
    if len(head) < 4:
        return None
    size = struct.unpack('>I', head)[0]
    body = sys.stdin.buffer.read(size)
    if len(body) < size:
        return None
    return json.loads(body)


hello = read_frame()
if hello is None:
    sys.exit(0)
body = json.dumps({'hello_ok': {'protocol': int(sys.argv[2]), 'store': 'silent-store', 'now_millis': 0}}).encode()
sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
sys.stdout.buffer.flush()
while True:
    time.sleep(1)
";

/// A stand-in `ssh` that greets the computer and then says nothing, which is one
/// shape of a lost link: the pipes stay open and no frame comes.
fn silent_stand_in(directory: &Path, starts: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("silent.py");
    fs::write(&relay, SILENT_RELAY).expect("the silent relay of the stand-in");
    let script = format!(
        "exec /usr/bin/env python3 '{}' '{}' '{}'",
        relay.display(),
        starts.display(),
        horizon_sync::SERVER_PROTOCOL_VERSION
    );
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the silent stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// A stand-in `ssh` that writes one line to its standard error, as `ssh` writes
/// the line of the failure that it met, and ends with `code`.
fn failing_stand_in(directory: &Path, line: &str, code: i32) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let script = if line.trim().is_empty() {
        format!("exit {code}")
    } else {
        format!("printf '%s\\n' '{line}' >&2\nexit {code}")
    };
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// How many times the stand-in of a link started.
fn start_count(starts: &Path) -> usize {
    fs::read_to_string(starts).map_or(0, |text| text.lines().count())
}

/// The arguments of the last call of the stand-in, once it wrote them. The file
/// exists before its text does, so a test waits for the content.
fn arguments(directory: &Path) -> Vec<String> {
    let path = directory.join("ssh-arguments.txt");
    eventually("the arguments of the stand-in ssh", || {
        let text = fs::read_to_string(&path).unwrap_or_default();
        (!text.trim().is_empty()).then_some(text)
    })
    .lines()
    .map(str::to_owned)
    .collect()
}

/// One server of a temporary home, which a test can stop and start again over the
/// same store.
struct Server {
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
        Self { socket, store, running }
    }

    /// Stops answering, as a stopped server process does: the socket goes and the
    /// open links end.
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

    /// Answers with the store at `path` on the socket of this server: the store that
    /// a person restored over the file of the server, or another store of the same
    /// person.
    fn serve_store(&mut self, path: &Path) {
        self.stop();
        self.store = Store::open(path).expect("the store of this path");
        self.restart();
    }

    /// Copies this store, with the content of its log, to `path`: the file that a
    /// person keeps as a copy of the server.
    fn copy_store_to(&self, path: &Path) {
        let connection = rusqlite::Connection::open(self.store.path()).expect("a reader of the store");
        connection
            .execute("VACUUM INTO ?1", [path.to_string_lossy().into_owned()])
            .expect("the copy of the store");
    }

    /// The machine records of the store, as a pull reports them.
    fn machines(&self) -> Vec<Machine> {
        self.store
            .pull(0, "")
            .expect("the rows of the server")
            .rows
            .into_iter()
            .filter(|row| !row.deleted && row.kind == "machine")
            .map(|row| serde_json::from_value(row.body).expect("a machine record"))
            .collect()
    }

    fn has(&self, name: &str) -> bool {
        self.machines().iter().any(|machine| machine.name == name)
    }
}

/// One computer of the test: its own machine directory, its supervisor and the
/// store that the UI loads.
struct Computer {
    machines_dir: PathBuf,
    supervisor: Option<Supervisor>,
    store: SyncedMachineStore,
}

impl Computer {
    fn start(machines_dir: &Path, socket: &Path) -> Self {
        Self::start_with_settings(machines_dir, socket, None)
    }

    /// A computer whose shared settings follow its server, from the store at
    /// `settings_path`.
    fn start_with_settings(machines_dir: &Path, socket: &Path, settings_path: Option<PathBuf>) -> Self {
        let stand_in_dir = machines_dir.join("stand-in");
        fs::create_dir_all(machines_dir).expect("the machine directory");
        let keys = horizon_core::MachineKeys::new(stand_in_dir.join("machine-keys"));
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", machines_dir, keys_only(keys))
                .expect("the settings of the link");
        config.program = stand_in(&stand_in_dir, socket);
        config.settings_path = settings_path;
        let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
        let store = SyncedMachineStore::new(supervisor.handle().clone());
        Self {
            machines_dir: machines_dir.to_path_buf(),
            supervisor: Some(supervisor),
            store,
        }
    }

    fn handle(&self) -> &horizon_sync::SyncHandle {
        self.supervisor.as_ref().expect("a running supervisor").handle()
    }

    fn stop(&mut self) {
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.stop();
        }
    }

    fn records(&self) -> Vec<Machine> {
        self.store.load().expect("the list of this computer").records
    }

    fn names(&self) -> Vec<String> {
        self.records().into_iter().map(|machine| machine.name).collect()
    }

    fn pending(&self) -> BTreeSet<String> {
        self.handle().pending()
    }

    /// The snapshot that a frame of the window reads.
    fn snapshot(&self) -> std::sync::Arc<horizon_sync::LinkSnapshot> {
        self.handle().snapshot()
    }

    fn state(&self) -> LinkState {
        self.handle().state()
    }

    /// Adds a machine as the sidebar or an agent does: the list of this computer
    /// changes at once, and the intent waits for the server.
    fn add(&self, name: &str, host: &str) {
        let mut records = self.records();
        records.push(machine(name, host));
        self.store.save(&records).expect("the intent of the add");
    }

    fn remove(&self, name: &str) {
        let records: Vec<Machine> = self
            .records()
            .into_iter()
            .filter(|machine| machine.name != name)
            .collect();
        self.store.save(&records).expect("the intent of the removal");
    }

    fn directory(&self) -> PathBuf {
        self.machines_dir.join("stand-in")
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

/// Waits until a computer is in step with the server.
fn in_step(computer: &Computer) {
    eventually("the link of a computer", || {
        matches!(computer.state(), LinkState::InStep { .. }).then_some(())
    });
}

#[test]
fn a_machine_of_one_computer_arrives_at_the_other_and_a_removal_comes_back() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);

    a.add("build-1", "203.0.113.10");
    // The server holds the machine before the link of this computer reads the
    // answer that commits the intent, and the other computer pulls the row in
    // between, so the test waits for both rather than for one of them.
    eventually("the machine at both computers and the committed intent", || {
        (b.names().contains(&"build-1".to_string()) && a.pending().is_empty()).then_some(())
    });
    assert_eq!(server.machines().len(), 1, "the server holds the machine once");
    assert!(
        server.machines()[0].added_at_millis > 1,
        "the server stamps the record with its own clock, not the clock of the computer"
    );

    b.remove("build-1");
    eventually("the removal at both computers", || {
        (a.names().is_empty() && b.names().is_empty()).then_some(())
    });
    assert!(
        server.machines().is_empty(),
        "the row of the server says that it is gone"
    );

    let called = arguments(&a.directory());
    for expected in [
        "-T",
        "BatchMode=yes",
        "ConnectTimeout=5",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=2",
        "server-vm",
        "/opt/horizon --server-link",
    ] {
        assert!(
            called.iter().any(|argument| argument == expected),
            "the ssh call must hold {expected}: {called:?}"
        );
    }
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_change_that_waits_for_an_away_server_is_pending_and_settles_when_it_returns() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.add("build-1", "203.0.113.10");
    // The other computer pulls the row as soon as the server commits it, and the
    // answer that settles the intent of this computer can arrive after that, so
    // the test waits for the settled queue too. A change that the server did not
    // decide is a change that waits, and it counts in the sidebar beside the
    // change that the person makes after the server went away.
    eventually("the machine at both computers and the settled queue", || {
        (a.names().len() == 1 && b.names().len() == 1 && a.pending().is_empty()).then_some(())
    });

    server.stop();
    eventually("the link that learns that the server is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });
    a.add("build-2", "203.0.113.11");
    eventually("the machine that waits for the server", || {
        (a.pending() == BTreeSet::from(["build-2".to_string()])).then_some(())
    });
    assert_eq!(
        a.names(),
        vec!["build-1".to_string(), "build-2".to_string()],
        "this computer keeps its own change beside its last list"
    );
    assert_eq!(
        b.names(),
        vec!["build-1".to_string()],
        "the other computer keeps its last list"
    );
    assert_eq!(
        a.state(),
        LinkState::Away { pending: 1 },
        "the sidebar says how many changes wait"
    );

    server.restart();
    eventually("the change that settles on both computers", || {
        (a.pending().is_empty() && b.names().contains(&"build-2".to_string())).then_some(())
    });
    a.stop();
    b.stop();
    server.stop();
}

/// The count of the sidebar is the number of intents that the server did not
/// decide, one for each change that the person made while it was away.
#[test]
fn each_change_that_the_server_did_not_decide_counts_one() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);
    assert!(a.pending().is_empty(), "a link in step has no intent that waits");

    server.stop();
    eventually("the link that learns that the server is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });
    assert_eq!(
        a.state(),
        LinkState::Away { pending: 0 },
        "an away server counts no change before the person makes one"
    );

    a.add("build-1", "203.0.113.10");
    eventually("the first change that waits", || {
        (a.state() == LinkState::Away { pending: 1 }).then_some(())
    });
    a.add("build-2", "203.0.113.11");
    eventually("the second change that waits", || {
        (a.state() == LinkState::Away { pending: 2 }).then_some(())
    });
    assert_eq!(
        a.pending(),
        BTreeSet::from(["build-1".to_string(), "build-2".to_string()]),
        "the names that count are the names of the queue"
    );

    server.restart();
    eventually("the two changes that settle", || {
        (a.pending().is_empty() && server.has("build-2")).then_some(())
    });
    a.stop();
    server.stop();
}

#[test]
fn the_snapshot_of_a_link_follows_a_push_a_pull_an_away_server_and_a_returned_one() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    eventually("a snapshot in step with the server", || {
        matches!(a.snapshot().state(), LinkState::InStep { .. }).then_some(())
    });
    let generation_before = a.snapshot().generation();

    // A push of this computer: the snapshot shows the intent until the server
    // decides it, and then it shows the record that the server answered.
    a.add("build-1", "203.0.113.10");
    eventually("the machine of the first computer in the list of the second", || {
        b.names().contains(&"build-1".to_string()).then_some(())
    });
    eventually("the settled queue in the snapshot", || {
        (a.snapshot().pending().is_empty() && a.snapshot().generation() > generation_before).then_some(())
    });
    assert!(
        a.snapshot().has_machine_key("studio") == a.handle().has_machine_key("studio"),
        "the key names of the snapshot are the names of this computer"
    );

    // The server goes: the snapshot says that it is away, and then how many
    // changes wait for it.
    server.stop();
    eventually("a snapshot that says the server is away", || {
        matches!(a.snapshot().state(), LinkState::Away { .. }).then_some(())
    });
    a.add("build-2", "203.0.113.11");
    eventually("the name that waits in the snapshot", || {
        a.snapshot().pending().contains("build-2").then_some(())
    });
    assert_eq!(
        a.snapshot().state(),
        &LinkState::Away { pending: 1 },
        "the snapshot says how many changes wait"
    );

    // The server returns, and the snapshot settles without a frame of the window.
    server.restart();
    eventually("the snapshot that settles", || {
        (a.snapshot().pending().is_empty() && matches!(a.snapshot().state(), LinkState::InStep { .. })).then_some(())
    });
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn the_queue_of_a_computer_survives_its_restart_while_the_server_is_away() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);
    in_step(&b);

    server.stop();
    eventually("the link of the first computer that is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });
    a.add("build-3", "203.0.113.12");
    assert!(a.pending().contains("build-3"));
    a.stop();

    // A start of the same computer while the server is still away: the queue and
    // the last list are on its disk.
    let mut a = Computer::start(&a.machines_dir, &server.socket);
    assert_eq!(a.names(), vec!["build-3".to_string()], "the list is kept");
    assert_eq!(
        a.pending(),
        BTreeSet::from(["build-3".to_string()]),
        "the intent that waits is kept"
    );

    server.restart();
    eventually("the change of the restarted computer at the other one", || {
        (b.names().contains(&"build-3".to_string()) && a.pending().is_empty()).then_some(())
    });
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn the_first_link_keeps_the_local_records_that_the_server_lacks() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    // A second computer already put one machine with a name that this computer
    // also has, and the server record is the one that must stay.
    let mut first = Computer::start(&home.path().join("first"), &server.socket);
    in_step(&first);
    first.add("shared", "198.51.100.7");
    eventually("the shared machine of the server", || {
        server.has("shared").then_some(())
    });
    let server_record = server.machines().into_iter().find(|machine| machine.name == "shared");

    // The computer that followed no server wrote its own list into its file store.
    let machines_dir = home.path().join("upgrading");
    fs::create_dir_all(&machines_dir).expect("the machine directory");
    let registry = horizon_core::MachineRegistry::new(machines_dir.clone());
    let file_store = horizon_core::Machines::with_records(
        registry,
        vec![machine("own", "203.0.113.20"), machine("shared", "203.0.113.21")],
    );
    file_store.save().expect("the list before the server");

    let mut upgrading = Computer::start(&machines_dir, &server.socket);
    eventually("the local record at the server", || server.has("own").then_some(()));
    eventually("the first link at both computers", || {
        let names = upgrading.names();
        (names.contains(&"own".to_string()) && names.contains(&"shared".to_string())).then_some(())
    });
    let shared = upgrading
        .records()
        .into_iter()
        .find(|machine| machine.name == "shared")
        .expect("the shared machine");
    assert_eq!(
        Some(shared),
        server_record,
        "a name that both have keeps the record of the server"
    );
    // The intents of the first link settle with the answers that commit them,
    // and a row that the other computer holds arrives in the same answers.
    eventually("the settled queue of the first link", || {
        upgrading.pending().is_empty().then_some(())
    });
    upgrading.stop();
    first.stop();
    server.stop();
}

#[test]
fn a_machine_that_only_arrived_from_the_server_has_no_key_on_this_computer() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);

    a.add("build-1", "203.0.113.10");
    eventually("the machine at the second computer", || {
        (b.names().len() == 1).then_some(())
    });
    assert!(
        b.handle().is_foreign("build-1"),
        "the record of the other computer is not one that this one wrote"
    );
    assert!(
        !a.handle().is_foreign("build-1"),
        "a record that this computer wrote needs no notice"
    );

    b.add("build-2", "203.0.113.11");
    eventually("the second machine", || (b.names().len() == 2).then_some(()));
    assert!(
        !b.handle().is_foreign("build-2"),
        "a machine that this computer adds itself is its own"
    );
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_machine_list_without_a_server_starts_no_child() {
    let home = tempfile::tempdir().expect("a directory");
    let machines_dir = home.path().join("machines");
    fs::create_dir_all(&machines_dir).expect("the machine directory");
    let registry = horizon_core::MachineRegistry::new(machines_dir);
    let file_store = horizon_core::Machines::with_records(registry, vec![machine("build-1", "203.0.113.10")]);
    file_store.save().expect("the list of one computer");

    let machines = horizon_core::Machines::load(horizon_core::MachineRegistry::new(home.path().join("machines")))
        .expect("the file store loads");
    assert_eq!(machines.records().len(), 1);
    assert!(
        !home.path().join("machines").join("ssh").exists(),
        "no child of a link is started without a server"
    );
}

/// The relay of an `ssh` whose server refuses the greeting: it reads the hello,
/// answers the error of `sys.argv[1]`, and ends the link. It adds one line to its
/// starts file for every start, so a test sees that no other attempt ran.
const REFUSING_RELAY: &str = "\
import json
import struct
import sys

with open(sys.argv[2], 'a') as starts:
    starts.write('start\\n')

head = sys.stdin.buffer.read(4)
if len(head) < 4:
    sys.exit(0)
size = struct.unpack('>I', head)[0]
hello = sys.stdin.buffer.read(size)
if len(hello) < size:
    sys.exit(0)
body = json.dumps({'error': {'code': sys.argv[1], 'message': 'This computer was removed from the server.'}})
body = body.encode()
sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
sys.stdout.buffer.flush()
";

/// A stand-in `ssh` whose server refuses the greeting with one code of the link.
fn refusing_stand_in(directory: &Path, code: &str, starts: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("refusing.py");
    fs::write(&relay, REFUSING_RELAY).expect("the refusing relay of the stand-in");
    let script = format!(
        "exec /usr/bin/env python3 '{}' '{}' '{}'",
        relay.display(),
        code,
        starts.display()
    );
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the refusing stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// A greeting that the server refuses is the end of the link and not a lost one: no
/// attempt can bring a token back, so the computer reads the reason and starts no
/// other child. A removal that a person made while this computer was away reaches
/// it here, with the record that the server kept of the removal.
#[test]
fn a_refused_greeting_ends_the_link_and_starts_no_other_attempt() {
    for (code, refused) in [
        ("removed", LinkState::Removed),
        ("not_logged_in", LinkState::NotLoggedIn),
    ] {
        let home = tempfile::tempdir().expect("a directory");
        let machines_dir = home.path().join("machines");
        fs::create_dir_all(&machines_dir).expect("the machine directory");
        let starts_file = home.path().join("starts.txt");
        let keys = horizon_core::MachineKeys::new(machines_dir.join("machine-keys"));
        let mut config = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            &machines_dir,
            keys_only(keys),
        )
        .expect("the settings of the link");
        config.program = refusing_stand_in(&home.path().join("stand-in"), code, &starts_file);
        let supervisor = Supervisor::start(config, || {}).expect("the supervisor");

        eventually("the state of a refused greeting", || {
            (supervisor.handle().state() == refused).then_some(())
        });
        // A lost link reports `away` at once and starts another child at its delay,
        // so a state that stays is the answer of the server and not the delay of a
        // retry.
        thread::sleep(Duration::from_millis(300));
        assert_eq!(
            supervisor.handle().state(),
            refused,
            "the refusal of the server is the state of the link, and not a lost link"
        );
        assert_eq!(start_count(&starts_file), 1, "a refusal starts no other child");
        supervisor.stop();
    }
}

/// A link that answers the hello and then nothing is lost at its bound: the
/// supervisor ends and reaps the child, reports `away`, and starts the next
/// attempt. The bounds of the test are short, so a link that no answer reached
/// shows in a second instead of a minute.
#[test]
fn a_link_that_stops_answering_is_reported_away_at_its_bound() {
    let home = tempfile::tempdir().expect("a directory");
    let machines_dir = home.path().join("machines");
    fs::create_dir_all(&machines_dir).expect("the machine directory");
    let starts_file = home.path().join("starts.txt");
    let bounds = LinkBounds {
        request: Duration::from_millis(400),
        wait: Duration::from_millis(800),
    };
    let keys = horizon_core::MachineKeys::new(machines_dir.join("machine-keys"));
    let mut config = SupervisorConfig::for_server(
        "server-vm",
        "/opt/horizon --server-link",
        &machines_dir,
        keys_only(keys),
    )
    .expect("the settings of the link");
    config.program = silent_stand_in(&home.path().join("stand-in"), &starts_file);
    config.bounds = bounds;
    let supervisor = Supervisor::start(config, || {}).expect("the supervisor");

    // The hello of the stand-in is answered, so the link is up before it goes
    // silent. The state of a start is `away` at once, so this is what says that
    // the next `away` is a lost link and not the state of a fresh supervisor.
    eventually("the greeted link of the stand-in", || {
        matches!(
            supervisor.handle().state(),
            LinkState::Behind | LinkState::InStep { .. }
        )
        .then_some(())
    });
    // The link times the bound of its request from the instant that the request
    // left, and it publishes that instant with the `Away` state. A test that read
    // the start of an open request found none under load, because the request of the
    // first turn leaves after the greeting, and it could also read the start of a
    // request of a later attempt: this wait reads the pair that the link published.
    let sent = eventually("the away state of the link with the request that it lost", || {
        supervisor.handle().lost_request_since()
    });
    let lost = sent.elapsed();
    assert!(
        lost >= bounds.request,
        "a link is not lost before its bound of {:?}: {lost:?}",
        bounds.request
    );
    // The upper bound is a budget of the test: it says that the link is lost by
    // itself, and a computer that runs this suite beside other work can take
    // seconds to observe it.
    assert!(
        lost < Duration::from_secs(30),
        "a silent link is lost by itself and not by a later attempt: {lost:?}"
    );

    // B15: the reason that the window shows. The link was open, so `ssh` answered
    // its keepalives while the program of the server stayed silent.
    eventually("the reason of the silent link", || {
        (supervisor.handle().snapshot().refusal() == Some(LinkRefusal::Silent)).then_some(())
    });

    // The child of the lost link is ended, and the next attempt starts another
    // one: a child that nobody reaped would stay beside the zombie of the last.
    eventually("the second start of the stand-in", || {
        (start_count(&starts_file) >= 2).then_some(())
    });
    supervisor.stop();
}

/// B15: an attempt that did not open the link leaves the reason that a person
/// reads, so `Server away` is not the same three words for a refused address, a
/// name that does not resolve, a key that changed, a refused login and a far side
/// without the command of the link.
#[test]
fn a_link_that_did_not_open_names_the_reason_of_its_last_attempt() {
    let home = tempfile::tempdir().expect("a directory");
    for (line, code, expected) in [
        (
            "ssh: connect to host 203.0.113.10 port 22: Connection timed out",
            255,
            LinkRefusal::Address,
        ),
        (
            "ssh: Could not resolve hostname vps.example: Name or service not known",
            255,
            LinkRefusal::Name,
        ),
        (
            "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!",
            255,
            LinkRefusal::HostKey,
        ),
        (
            "deploy@203.0.113.10: Permission denied (publickey).",
            255,
            LinkRefusal::Login,
        ),
        ("", 127, LinkRefusal::Command),
    ] {
        let machines_dir = home.path().join(format!("machines-{}", expected.code()));
        fs::create_dir_all(&machines_dir).expect("the machine directory");
        let keys = horizon_core::MachineKeys::new(machines_dir.join("machine-keys"));
        let mut config = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            &machines_dir,
            keys_only(keys),
        )
        .expect("the settings of the link");
        config.program = failing_stand_in(&home.path().join(format!("stand-in-{}", expected.code())), line, code);
        let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
        let handle = supervisor.handle().clone();

        let reason = eventually("the reason of the failed attempt", || handle.snapshot().refusal());
        assert_eq!(reason, expected, "the window names the reason of `{line}`");
        assert!(
            matches!(handle.state(), LinkState::Away { .. }),
            "a reason travels with the state that a person reads"
        );
        assert_eq!(
            horizon_sync::of_ssh_line(line),
            (!line.trim().is_empty()).then_some(expected),
            "the words of the child are what names the reason"
        );
        supervisor.stop();
    }
}

/// The state of a computer that is idle when the server goes: it must learn that
/// the server is away, and it must be in step again with the change that the
/// other computer made while the server was away.
#[test]
fn a_computer_that_is_idle_learns_that_the_server_is_gone_and_catches_up() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &server.socket);
    in_step(&a);
    in_step(&b);
    a.add("build-1", "203.0.113.10");
    // The queue of this computer settles with the answer that commits the row,
    // and the other computer pulls the row in between, so the wait holds both.
    eventually("the first machine of both computers and the settled queue", || {
        (b.names() == vec!["build-1".to_string()] && a.pending().is_empty()).then_some(())
    });
    in_step(&b);

    // B is idle: its wait is open, and nothing of this computer writes.
    server.stop();
    eventually("the idle computer that learns that the server is away", || {
        matches!(b.state(), LinkState::Away { .. }).then_some(())
    });
    a.add("build-2", "203.0.113.11");
    eventually("the change that waits for the server", || {
        (a.pending() == BTreeSet::from(["build-2".to_string()])).then_some(())
    });
    assert_eq!(
        b.names(),
        vec!["build-1".to_string()],
        "the idle computer keeps its last list while the server is away"
    );

    server.restart();
    eventually("both computers in step with the server", || {
        (matches!(a.state(), LinkState::InStep { .. }) && matches!(b.state(), LinkState::InStep { .. })).then_some(())
    });
    assert_eq!(
        b.names(),
        vec!["build-1".to_string(), "build-2".to_string()],
        "the idle computer catches up with the change of the other one"
    );
    let mut server_names: Vec<String> = server.machines().into_iter().map(|machine| machine.name).collect();
    server_names.sort();
    assert_eq!(
        b.names(),
        server_names,
        "the mirror of an idle computer equals the server"
    );
    assert!(a.pending().is_empty(), "the change that waited is settled");
    a.stop();
    b.stop();
    server.stop();
}

/// B4: a question of the Server tab answers at once while the link is away. The
/// window reads one only inside an open link, so a question asked during a cut
/// waited for an attempt that the delays of a lost link decide: the sentence of
/// `Add a computer` stood there and its button stayed disabled.
#[test]
fn a_question_of_the_window_answers_at_once_when_the_server_is_away() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);

    server.stop();
    eventually("the link that learns that the server is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });

    for request in [
        ServerRequest::Invite,
        ServerRequest::Devices,
        ServerRequest::Remove("mbp-7c1".to_owned()),
    ] {
        let asked = a.handle().ask(request.clone());
        let answer = asked
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("{request:?} answered no sentence: {error}"));
        assert_eq!(
            answer,
            ServerReply::Failed("This computer has no link to the server now.".to_owned()),
            "{request:?} answers the sentence of the Server tab at once"
        );
    }

    a.stop();
    server.stop();
}

/// A stand-in `ssh` that never greets: the far side is a program that says
/// nothing, so the link of the test is away for as long as the test runs.
fn sleeping_stand_in(directory: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let program = horizon_core::stand_in_program_in(directory, "ssh", "sleep 600").expect("the stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// B16: the queue of a computer that is away for hours. The audit found no cap and
/// asked for the numbers that a cap would be chosen from, so this test holds ten
/// thousand intents and measures the file, the read of a start and the drain.
///
/// The numbers say that no cap is needed at this volume. The file stays at a few
/// megabytes, a start reads it in a moment, and the drain leaves in pushes of at
/// most [`MAX_INTENTS_PER_PUSH`] intents, so one push never grows past its frame.
/// A cap would refuse a change that a person made while the server was away, and
/// that loses the change instead of holding it. The count of what waits is already
/// in the sidebar (`Server away · N pending`) and on each row of the machine list,
/// so the person sees the queue grow without a second line.
#[test]
fn the_queue_of_a_computer_that_is_away_for_hours_needs_no_cap() {
    const INTENTS: usize = 10_000;

    let home = tempfile::tempdir().expect("a directory");
    let machines_dir = home.path().join("machines");
    fs::create_dir_all(&machines_dir).expect("the machine directory");
    let keys = horizon_core::MachineKeys::new(machines_dir.join("machine-keys"));
    let mut config = SupervisorConfig::for_server(
        "server-vm",
        "/opt/horizon --server-link",
        &machines_dir,
        keys_only(keys),
    )
    .expect("the settings of the link");
    config.program = sleeping_stand_in(&home.path().join("stand-in"));
    let queue_file = config.queue_file.clone();
    let device = config.device.clone();
    let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
    let store = SyncedMachineStore::new(supervisor.handle().clone());

    // One save of the whole machine list, as the sidebar and an agent's operation
    // write it: ten thousand records that the server does not hold yet.
    let records: Vec<Machine> = (0..INTENTS)
        .map(|index| machine(&format!("build-{index:05}"), "203.0.113.10"))
        .collect();
    let saved_at = Instant::now();
    store.save(&records).expect("the intents of one save");
    let saved = saved_at.elapsed();

    // The link of the test never greets, so this computer stops with its queue
    // whole. A start then reads the file with no second writer on it.
    supervisor.stop();

    let bytes = fs::metadata(&queue_file).expect("the queue file").len();
    let started = Instant::now();
    let mut client = Client::open(queue_file, &device).expect("the queue of a start");
    let loaded = started.elapsed();
    assert_eq!(
        client.pending().len(),
        INTENTS,
        "every change that waits for an away server is held whole"
    );

    let mut chunks = 0;
    let mut drained = 0;
    let drain_at = Instant::now();
    while let Some(chunk) = client.next_push() {
        assert!(
            chunk.len() <= MAX_INTENTS_PER_PUSH,
            "one push holds at most {MAX_INTENTS_PER_PUSH} intents"
        );
        let last = chunk.last().expect("a push of at least one intent").id;
        let reply = PushReply {
            results: chunk
                .iter()
                .map(|intent| IntentResult {
                    id: intent.id,
                    outcome: Outcome::Applied { index: intent.id },
                })
                .collect(),
            next_expected: last.saturating_add(1),
        };
        client.accept_push(&reply).expect("the answer of a push");
        drained += chunk.len();
        chunks += 1;
        assert!(chunks <= INTENTS, "the drain ends in one push for each chunk");
    }
    let drain = drain_at.elapsed();

    assert_eq!(drained, INTENTS, "the drain leaves no change behind");
    assert_eq!(
        chunks,
        INTENTS.div_ceil(MAX_INTENTS_PER_PUSH),
        "the drain is chunked by the bound of one push"
    );
    assert!(
        bytes < 8 * 1024 * 1024,
        "{INTENTS} intents are {bytes} bytes, and a start reads them"
    );
    // The bound is generous on purpose: this is a measurement of a queue at ten
    // thousand intents, and a computer that does other work at the same time
    // writes and syncs the file at a slower pace. The finding would be a drain
    // that takes minutes, because that is when a person would see a stuck queue.
    assert!(
        saved < Duration::from_secs(120) && loaded < Duration::from_secs(5) && drain < Duration::from_secs(120),
        "the save took {saved:?}, a start read {bytes} bytes in {loaded:?} and the drain took {drain:?}"
    );
}

/// The files of `directory` whose name starts with `prefix`.
fn dated_files(directory: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect();
    files.sort();
    files
}

/// A person who restores or moves an older copy of the file of the server makes the
/// log of the server end below the index that this computer saw. The computer is the
/// newer side of that log for the records that the copy does not hold: it keeps them,
/// it sends them to the server again, and one dated file of its home holds them.
#[test]
fn a_restored_older_copy_of_the_store_keeps_the_records_that_the_server_lacks() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &server.socket);
    in_step(&a);
    a.add("base-1", "203.0.113.10");
    eventually("the first machine at the server", || server.has("base-1").then_some(()));

    // The copy of the store that a person keeps, before the change that only the
    // newer state has.
    let copy = home.path().join("copy").join("server.sqlite");
    fs::create_dir_all(copy.parent().expect("a folder of the copy")).expect("the folder of the copy");
    server.copy_store_to(&copy);
    a.add("newer-1", "203.0.113.11");
    eventually("the newer machine at the server", || {
        server.has("newer-1").then_some(())
    });

    // The person restores the older copy, and the server answers with it: its log
    // ends below the index that this computer saw.
    server.serve_store(&copy);
    eventually("the machine that only the newer state had, at the server again", || {
        server.has("newer-1").then_some(())
    });
    assert!(server.has("base-1"), "the machine of the copy is still there");
    eventually("the machine list of this computer", || {
        (a.names().len() == 2).then_some(())
    });
    assert!(a.names().contains(&"newer-1".to_string()), "{:?}", a.names());

    let lines = a.handle().take_notices();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("older state than this computer saw")),
        "the person reads the case: {lines:?}"
    );
    let kept = dated_files(&a.machines_dir, "server-before-reset.");
    assert_eq!(kept.len(), 1, "one dated file holds what this computer had: {kept:?}");
    let text = fs::read_to_string(&kept[0]).expect("the kept machine list");
    assert!(text.contains("older-store"), "the file names the case: {text}");
    for name in ["base-1", "newer-1"] {
        assert!(text.contains(name), "the file holds {name}: {text}");
    }
    assert!(
        lines.iter().any(|line| line.contains("server-before-reset.")),
        "the line names the file that holds the list: {lines:?}"
    );
    a.stop();
    server.stop();
}

/// A person who points the server at another store gives this computer the state of a
/// store that it did not see. The machine list and the settings of this computer are
/// kept in one dated file each before the answer of that store applies, and one line
/// names both files.
#[test]
fn another_store_keeps_the_machine_list_and_the_settings_of_this_computer() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let machine_home = home.path().join("a");
    fs::create_dir_all(&machine_home).expect("the home of this computer");
    let settings_path = machine_home.join("config.json");
    let mut settings = Config::default();
    settings.appearance.theme = AppearanceTheme::Light;
    Config::save(&settings_path, &settings).expect("the settings of this computer");
    let mut a = Computer::start_with_settings(&machine_home, &server.socket, Some(settings_path));
    in_step(&a);
    a.add("base-1", "203.0.113.10");
    eventually("the machine at the server", || server.has("base-1").then_some(()));

    // The server answers with a store that this computer did not see: a file that a
    // person made at another start of a server.
    let other = home.path().join("other").join("server.sqlite");
    fs::create_dir_all(other.parent().expect("a folder of the other store")).expect("the folder of the other store");
    server.serve_store(&other);

    eventually("the line of another store", || {
        a.handle()
            .take_notices()
            .iter()
            .any(|line| line.contains("another store") && line.contains("server-before-reset."))
            .then_some(())
    });
    let kept = dated_files(&machine_home, "server-before-reset.");
    assert_eq!(kept.len(), 1, "one dated file holds the machine list: {kept:?}");
    let machines = fs::read_to_string(&kept[0]).expect("the kept machine list");
    assert!(machines.contains("another-store"), "{machines}");
    assert!(
        machines.contains("base-1"),
        "the file holds the machine list: {machines}"
    );

    let backup = dated_files(&machine_home, "settings.before-server.");
    assert_eq!(backup.len(), 1, "one backup holds the settings: {backup:?}");
    let settings = fs::read_to_string(&backup[0]).expect("the backup of the settings");
    assert!(
        settings.contains("appearance"),
        "the backup holds the sections: {settings}"
    );
    assert!(
        settings.contains("light"),
        "the backup holds the value of this computer: {settings}"
    );
    a.stop();
    server.stop();
}
