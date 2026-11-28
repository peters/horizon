//! A real Unix socket of this computer, the card contract on its far end, the
//! `ssh` child that carries it, and the stop that leaves no thread, no socket and
//! no child behind.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use horizon_core::machine_watch::connection;
use horizon_core::node_cards;
use horizon_core::{Machine, MachineKeys, MachineRegistry, Machines, SshConnection, stand_in_program_in};

use super::{CardMachine, NodeCards};

const HOST: &str = "0123456789abcdef0123456789abcdef";
const WAIT: Duration = Duration::from_secs(20);

/// One short temporary folder, because a Unix socket path is short on macOS.
fn scratch() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("hz-cards")
        .tempdir_in("/tmp")
        .expect("a private folder")
}

/// The card sockets of one test, in a folder that no other test uses.
fn cards_in(directory: &tempfile::TempDir) -> NodeCards {
    let mut cards = NodeCards::default();
    cards.base = directory.path().to_path_buf();
    cards
}

fn socket_file(cards: &NodeCards, machine: &str, session: &str) -> PathBuf {
    node_cards::local_dir(&cards.base)
        .join(machine)
        .join(format!("{session}.sock"))
}

/// A connection to the socket of `machine` and `session`, once the supervisor
/// bound it.
fn connect(cards: &NodeCards, machine: &str, session: &str) -> UnixStream {
    let socket = socket_file(cards, machine, session);
    let deadline = Instant::now() + WAIT;
    loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => return stream,
            Err(error) => assert!(
                Instant::now() < deadline,
                "no card socket at {}: {error}",
                socket.display()
            ),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The answer to `initialize` on the socket of `machine` and `session`.
fn initialize(cards: &NodeCards, machine: &str, session: &str) -> String {
    let stream = connect(cards, machine, session);
    stream.set_read_timeout(Some(WAIT)).expect("a read bound");
    let mut writer = stream.try_clone().expect("a second handle");
    writer
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"node-cards-test","version":"1"}}}
"#,
        )
        .expect("write initialize");
    writer.flush().expect("flush");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("a read");
    line
}

fn wait_gone(path: &Path) {
    let deadline = Instant::now() + WAIT;
    while path.exists() {
        assert!(Instant::now() < deadline, "{} stays", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until `wanted` holds, and names what the test waited for.
fn wait_until(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !wanted() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One machine record of a test, with the mark of a card setup.
fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "127.0.0.1".to_owned(),
        user: None,
        port: Some(2222),
        session: None,
        provider: None,
        repository: None,
        folder: None,
        added_at_millis: 0,
        cards: true,
    }
}

/// The login of a machine terminal of `name`, in the folder of a test.
fn login(folder: &Path, name: &str) -> SshConnection {
    let machines = Machines::with_records(MachineRegistry::new(folder.to_path_buf()), vec![record(name)]);
    let keys = MachineKeys::new(folder.join("machine-keys"));
    connection(&machines, &keys, name).expect("the login of a machine terminal")
}

/// The machine of a test with its sessions, and the login of a machine terminal.
fn machine_of(cards: &NodeCards, folder: &Path, name: &str, sessions: &[&str]) -> CardMachine {
    CardMachine {
        name: name.to_owned(),
        login: login(folder, name),
        sessions: sessions.iter().map(|session| (*session).to_owned()).collect(),
        refused: Vec::new(),
        links: super::Links::clone(&cards.links),
    }
}

/// One stand-in `ssh` of a test: the argv of each call it ran.
struct StandIn {
    folder: tempfile::TempDir,
    argv: PathBuf,
}

impl StandIn {
    /// The stand-in that holds its link, as a forward that `sshd` accepted does.
    /// It holds it in this shell, so no program of it outlives the end of the link.
    fn holding() -> Self {
        Self::of("case $* in *'mkdir -p '*) exit 0 ;; esac\nwhile :; do sleep 1; done\n")
    }

    /// The stand-in that ends at once with the words of a refused forward.
    fn refusing() -> Self {
        Self::of("case $* in *'mkdir -p '*) exit 0 ;; esac\nprintf 'remote port forwarding failed\n' >&2\nexit 255\n")
    }

    /// A stand-in of `body`, which answers the removal of the socket files of a
    /// machine at once unless it proves that removal itself.
    fn of(body: &str) -> Self {
        let folder = scratch();
        let argv = folder.path().join("argv");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {argv}\n{body}",
            argv = argv.display(),
        );
        stand_in_program_in(folder.path(), "ssh", &script).expect("the stand-in ssh of the test");
        Self { folder, argv }
    }

    /// The program of the stand-in.
    fn program(&self) -> PathBuf {
        self.folder.path().join("ssh")
    }

    /// How many times the supervisor started the child of a machine: the argv of each
    /// call, without the removal of the socket files that comes before each of them.
    fn calls(&self) -> usize {
        self.argv().lines().filter(|line| !line.contains("mkdir -p ")).count()
    }

    /// The command line of every call, as one text.
    fn argv(&self) -> String {
        std::fs::read_to_string(&self.argv).unwrap_or_default()
    }

    /// The command line of the child of the machine, once the supervisor started it.
    fn carried(&self) -> Option<String> {
        self.argv()
            .lines()
            .rev()
            .find(|line| line.split_whitespace().any(|word| word == "-R"))
            .map(str::to_owned)
    }

    /// Ends every program of this stand-in, and every program that those started.
    ///
    /// The stand-in of a holding forward waits for a link that ends with its
    /// window, so a test that ends before that link leaves the program behind.
    fn end_programs(&self) {
        crate::app::test_support::end_programs_holding(&[&self.folder.path().to_string_lossy()]);
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.end_programs();
    }
}

/// The words of the removal that Horizon runs before each start of the child of a
/// machine: the folder of its sockets, mode 700, and the socket file of each session.
fn prune_words(sessions: &[&str]) -> String {
    let folder = "/tmp/horizon-cards/build-1";
    let paths: Vec<String> = sessions
        .iter()
        .map(|session| format!("{folder}/{session}.sock"))
        .collect();
    format!(
        "mkdir -p /tmp/horizon-cards {folder} && chmod 700 /tmp/horizon-cards {folder} && rm -f -- {}",
        paths.join(" ")
    )
}

/// The body of one stand-in `ssh` that binds the socket file of each `-R` in
/// `folder`, as the `sshd` of a machine does, and that leaves each file behind when
/// it ends. A bind over a file that is still there, and a bind in a folder that is
/// not there, are the words and the status of a real `sshd` that refuses a path, and
/// `log` holds one line for each call, with the words of a removal of Horizon.
fn binding_body(folder: &Path, log: &Path) -> String {
    format!(
        "D={folder}\nL={log}\nbind=\nlast=\n\
while [ $# -gt 0 ]; do\n\
\x20 case $1 in\n\
\x20   -R) bind=\"$bind $2\"; shift 2; continue ;;\n\
\x20   -o|-p|-i) shift 2; continue ;;\n\
\x20   -*) shift; continue ;;\n\
\x20 esac\n\
\x20 last=$1; shift\ndone\n\
case $last in\n\
\x20 mkdir\\ -p*\" && chmod 700 \"*\" && rm -f -- \"*)\n\
\x20   printf 'prune %s\\n' \"$last\" >> \"$L\"\n\
\x20   mkdir -p \"$D\"; chmod 700 \"$D\"\n\
\x20   set -f; set -- ${{last#*rm -f -- }}; set +f\n\
\x20   for p in \"$@\"; do rm -f \"$D/$(basename \"$p\")\"; done\n\
\x20   exit 0 ;;\nesac\n\
for pair in $bind; do\n\
\x20 n=$(basename \"${{pair%%:*}}\")\n\
\x20 if [ ! -d \"$D\" ]; then printf 'no folder %s\\n' \"$D\" >> \"$L\"; exit 255; fi\n\
\x20 if [ -e \"$D/$n\" ]; then printf 'stale %s\\n' \"$n\" >> \"$L\"; exit 255; fi\n\
\x20 : > \"$D/$n\"\n\
\x20 printf 'bind %s\\n' \"$n\" >> \"$L\"\ndone\nsleep 0.4\nexit 255\n",
        folder = folder.display(),
        log = log.display(),
    )
}

/// The lines of the log of a stand-in `ssh`, once its program wrote them.
fn recorded(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// How many `ssh` children of `cards` carry its sockets now: the socket folder of
/// this process is in the argv of each of them.
fn children_of(cards: &NodeCards) -> usize {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "args="])
        .output()
        .expect("the process list of this computer");
    let marker = node_cards::local_dir(&cards.base).display().to_string();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

/// The card sockets of `machine` with a stand-in `ssh`: the sockets and the argv.
fn cards_with(directory: &tempfile::TempDir, stand_in: &StandIn) -> NodeCards {
    let mut cards = cards_in(directory);
    cards.program = stand_in.program().to_string_lossy().into_owned();
    // A child that ends starts the next one at once: a lane proves that the supervisor
    // tries again, and the length of the rule of a person is proven beside it.
    cards.delay = Some(Duration::from_millis(40));
    cards
}

#[test]
fn a_session_gets_one_socket_that_serves_the_card_contract() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    cards.set_sessions(vec![machine], HOST);
    assert!(cards.wanted.is_some());
    let answer = initialize(&cards, "build-1", "fix-71");
    assert!(answer.contains("serverInfo"), "{answer}");
    cards.stop_all();
}

#[test]
fn a_session_that_leaves_loses_its_socket_and_the_others_stay() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let both = machine_of(&cards, directory.path(), "build-1", &["fix-71", "night"]);
    cards.set_sessions(vec![both], HOST);
    initialize(&cards, "build-1", "night");
    let other = machine_of(&cards, directory.path(), "build-2", &["night"]);
    cards.set_sessions(vec![other], HOST);
    wait_gone(&socket_file(&cards, "build-1", "night"));
    let answer = initialize(&cards, "build-2", "night");
    assert!(answer.contains("serverInfo"), "{answer}");
    cards.stop_all();
}

#[test]
fn a_stop_leaves_no_thread_and_no_socket() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    cards.set_sessions(vec![machine], HOST);
    let socket = socket_file(&cards, "build-1", "fix-71");
    initialize(&cards, "build-1", "fix-71");
    assert!(socket.exists(), "the listener bound its socket");
    cards.stop_all();
    assert!(!socket.exists(), "a stop removes the socket file");
    assert!(
        !node_cards::local_dir(&cards.base).exists(),
        "a stop removes the folder"
    );
    assert!(cards.wanted.is_none());
}

#[test]
fn a_machine_name_outside_the_rule_opens_no_socket() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let mut machine = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    machine.name = "../build".to_owned();
    cards.set_sessions(vec![machine], HOST);
    // The supervisor names every skipped session in the log and opens nothing.
    std::thread::sleep(Duration::from_millis(200));
    assert!(!socket_file(&cards, "../build", "fix-71").exists());
    cards.stop_all();
}

#[test]
fn a_machine_carries_one_child_for_all_of_its_sessions() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["fix-71", "night"]);
    cards.set_sessions(vec![machine], HOST);
    wait_until("the child of the machine", || stand_in.carried().is_some());
    // The child of the two sessions of one machine carries one `-R` for each of them.
    let carried = stand_in.carried().expect("the child of the machine");
    assert_eq!(carried.matches("-R").count(), 2, "{carried}");
    assert!(carried.contains("-N") && carried.contains("-T"), "{carried}");
    assert!(carried.contains("StreamLocalBindUnlink=yes"), "{carried}");
    wait_until("the process of the child", || children_of(&cards) == 1);
    cards.stop_all();
    assert_eq!(children_of(&cards), 0, "a stop leaves no ssh child");
    assert!(!socket_file(&cards, "build-1", "night").exists());
}

#[test]
fn a_session_that_joins_ends_the_child_and_starts_the_next() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let one = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    cards.set_sessions(vec![one], HOST);
    wait_until("the child of one session", || stand_in.calls() == 1);
    let two = machine_of(&cards, directory.path(), "build-1", &["fix-71", "night"]);
    cards.set_sessions(vec![two], HOST);
    wait_until("the child of two sessions", || stand_in.calls() == 2);
    wait_until("one process for the machine", || children_of(&cards) == 1);
    cards.stop_all();
    assert_eq!(children_of(&cards), 0);
}

#[test]
fn a_machine_that_leaves_ends_its_child() {
    let stand_in = StandIn::holding();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["night"]);
    cards.set_sessions(vec![machine], HOST);
    wait_until("the child of the machine", || children_of(&cards) == 1);
    cards.set_sessions(vec![], HOST);
    wait_until("no child of the machine", || children_of(&cards) == 0);
    cards.stop_all();
}

#[test]
fn a_restarted_child_removes_the_socket_file_that_the_one_before_it_left() {
    let folder = scratch();
    let machine = folder.path().join("machine");
    std::fs::create_dir_all(&machine).expect("the folder of the sockets of a machine");
    let log = folder.path().join("log");
    let stand_in = StandIn::of(&binding_body(&machine, &log));
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["night"]);
    cards.set_sessions(vec![machine], HOST);
    wait_until("the second bind of the socket of the session", || {
        recorded(&log).matches("bind night.sock").count() == 2
    });
    let held = recorded(&log);
    assert!(
        !held.contains("stale"),
        "a bind found the file of the child before it: {held}"
    );
    let night = prune_words(&["night"]);
    assert!(
        held.starts_with(&format!(
            "prune {night}\nbind night.sock\nprune {night}\nbind night.sock\n"
        )),
        "each child removes the socket file of the one before it: {held}"
    );
    cards.stop_all();
}

#[test]
fn a_fresh_machine_gets_the_folder_of_its_sockets_before_the_first_bind() {
    // A machine whose folder is not there yet: `sshd` binds nothing in it until
    // Horizon makes it, and no other program of that machine makes it.
    let folder = scratch();
    let machine = folder.path().join("machine");
    let log = folder.path().join("log");
    let stand_in = StandIn::of(&binding_body(&machine, &log));
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let one = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    cards.set_sessions(vec![one], HOST);
    wait_until("the first bind of the socket of one session", || {
        recorded(&log).contains("bind fix-71.sock")
    });
    let held = recorded(&log);
    assert!(
        !held.contains("no folder"),
        "the folder of the machine was there: {held}"
    );
    let prunes: Vec<&str> = held.lines().filter_map(|line| line.strip_prefix("prune ")).collect();
    let one = prune_words(&["fix-71"]);
    assert_eq!(
        prunes.first().copied(),
        Some(one.as_str()),
        "the removal of one session: {held}"
    );
    let mode = std::fs::metadata(&machine)
        .expect("the folder of the machine")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "the folder of the machine of the removal is private");
    cards.stop_all();
}

#[test]
fn a_session_that_joins_makes_the_removal_name_both_socket_files() {
    let folder = scratch();
    let machine = folder.path().join("machine");
    let log = folder.path().join("log");
    let stand_in = StandIn::of(&binding_body(&machine, &log));
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let one = machine_of(&cards, directory.path(), "build-1", &["fix-71"]);
    cards.set_sessions(vec![one], HOST);
    wait_until("the child of one session", || {
        recorded(&log).contains("bind fix-71.sock")
    });
    let two = machine_of(&cards, directory.path(), "build-1", &["fix-71", "night"]);
    cards.set_sessions(vec![two], HOST);
    wait_until("the child of two sessions", || {
        recorded(&log).contains("bind night.sock")
    });
    let held = recorded(&log);
    let prunes: Vec<&str> = held.lines().filter_map(|line| line.strip_prefix("prune ")).collect();
    let (one, both) = (prune_words(&["fix-71"]), prune_words(&["fix-71", "night"]));
    let one_session = prunes.iter().position(|words| *words == one.as_str());
    let both_sessions = prunes.iter().position(|words| *words == both.as_str());
    assert!(
        matches!((one_session, both_sessions), (Some(one), Some(both)) if one < both),
        "the removal of one session, then of two: {held}"
    );
    assert!(
        !held.contains("stale"),
        "a bind found the file of the child before it: {held}"
    );
    cards.stop_all();
}

#[test]
fn a_refused_forward_waits_the_delay_and_tries_again() {
    let stand_in = StandIn::refusing();
    let directory = scratch();
    let mut cards = cards_with(&directory, &stand_in);
    let machine = machine_of(&cards, directory.path(), "build-1", &["night"]);
    cards.set_sessions(vec![machine], HOST);
    wait_until("the second attempt of a refused forward", || stand_in.calls() >= 2);
    wait_until("the refusal of the card link of the machine", || {
        cards.refusal("build-1").is_some()
    });
    cards.stop_all();
}
