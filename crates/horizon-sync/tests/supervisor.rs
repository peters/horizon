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
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use horizon_core::Machine;
use horizon_core::machines::MachineStore;
use horizon_sync::{
    LinkBounds, LinkState, SocketServer, Store, Supervisor, SupervisorConfig, SyncedMachineStore,
    run_server_on_unix_socket,
};

/// The time a test gives a change to travel through a link and a store.
const PATIENCE: Duration = Duration::from_secs(30);

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
fn keys_only(keys: horizon_core::MachineKeys) -> std::sync::Arc<dyn horizon_core::SecretTarget> {
    SupervisorConfig::keys_only(keys)
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
    let program = directory.join("ssh");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexec /usr/bin/env python3 '{}' '{}'\n",
        arguments.display(),
        relay.display(),
        socket.display()
    );
    fs::write(&program, script).expect("the stand-in ssh");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("an executable stand-in");
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
body = json.dumps({'hello_ok': {'protocol': 1, 'store': 'silent-store', 'now_millis': 0}}).encode()
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
    let program = directory.join("ssh");
    let script = format!(
        "#!/bin/sh\nexec /usr/bin/env python3 '{}' '{}'\n",
        relay.display(),
        starts.display()
    );
    fs::write(&program, script).expect("the silent stand-in ssh");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("an executable stand-in");
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
        let running = Some(run_server_on_unix_socket(&socket, store.clone()).expect("the socket"));
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
        self.running = Some(run_server_on_unix_socket(&self.socket, self.store.clone()).expect("the socket"));
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
        let stand_in_dir = machines_dir.join("stand-in");
        fs::create_dir_all(machines_dir).expect("the machine directory");
        let keys = horizon_core::MachineKeys::new(stand_in_dir.join("machine-keys"));
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", machines_dir, keys_only(keys))
                .expect("the settings of the link");
        config.program = stand_in(&stand_in_dir, socket);
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
    eventually("the machine of the first computer", || {
        b.names().contains(&"build-1".to_string()).then_some(())
    });
    assert_eq!(server.machines().len(), 1, "the server holds the machine once");
    assert!(a.pending().is_empty(), "a committed intent is not pending");
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
    eventually("the machine at both computers", || {
        (a.names().len() == 1 && b.names().len() == 1).then_some(())
    });

    server.stop();
    eventually("the link that learns that the server is away", || {
        matches!(a.state(), LinkState::Away { .. }).then_some(())
    });
    a.add("build-2", "203.0.113.11");
    eventually("the machine that waits for the server", || {
        a.pending().contains("build-2").then_some(())
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
    assert!(upgrading.pending().is_empty());
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
    let started = Instant::now();
    eventually("the link that is reported away", || {
        matches!(supervisor.handle().state(), LinkState::Away { .. }).then_some(())
    });
    let lost = started.elapsed();
    assert!(
        lost >= bounds.request,
        "a link is not lost before its bound of {:?}: {lost:?}",
        bounds.request
    );
    assert!(
        lost < Duration::from_secs(5),
        "a silent link is lost at its bound of {:?}, and not later: {lost:?}",
        bounds.request
    );

    // The child of the lost link is ended, and the next attempt starts another
    // one: a child that nobody reaped would stay beside the zombie of the last.
    eventually("the second start of the stand-in", || {
        (start_count(&starts_file) >= 2).then_some(())
    });
    supervisor.stop();
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
    eventually("the first machine of both computers", || {
        (b.names() == vec!["build-1".to_string()]).then_some(())
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
