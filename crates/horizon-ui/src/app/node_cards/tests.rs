//! A real Unix socket of this computer, the card contract on its far end, and
//! the stop that leaves no thread and no socket behind.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use horizon_core::node_cards;

use super::NodeCards;

const HOST: &str = "0123456789abcdef0123456789abcdef";
const WAIT: Duration = Duration::from_secs(20);

fn session(machine: &str, session: &str) -> (String, String) {
    (machine.to_owned(), session.to_owned())
}

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

#[test]
fn a_session_gets_one_socket_that_serves_the_card_contract() {
    let directory = scratch();
    let mut cards = cards_in(&directory);
    cards.set_sessions(&[session("build-1", "fix-71")], HOST);
    assert!(cards.wanted.is_some());
    let answer = initialize(&cards, "build-1", "fix-71");
    assert!(answer.contains("serverInfo"), "{answer}");
    cards.stop_all();
}

#[test]
fn a_session_that_leaves_loses_its_socket_and_the_others_stay() {
    let directory = scratch();
    let mut cards = cards_in(&directory);
    cards.set_sessions(&[session("build-1", "fix-71"), session("build-1", "night")], HOST);
    initialize(&cards, "build-1", "night");
    cards.set_sessions(&[session("build-2", "night")], HOST);
    wait_gone(&socket_file(&cards, "build-1", "night"));
    let answer = initialize(&cards, "build-2", "night");
    assert!(answer.contains("serverInfo"), "{answer}");
    cards.stop_all();
}

#[test]
fn a_stop_leaves_no_thread_and_no_socket() {
    let directory = scratch();
    let mut cards = cards_in(&directory);
    cards.set_sessions(&[session("build-1", "fix-71")], HOST);
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
    let directory = scratch();
    let mut cards = cards_in(&directory);
    cards.set_sessions(&[session("../build", "fix-71")], HOST);
    // The supervisor names every skipped session in the log and opens nothing.
    std::thread::sleep(Duration::from_millis(200));
    assert!(!socket_file(&cards, "../build", "fix-71").exists());
    cards.stop_all();
}
