//! The link over a Unix socket, and the relay that a later slice puts an `ssh`
//! child in front of.

use std::{
    fs,
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    thread,
    time::Duration,
};

use horizon_core::Machine;
use horizon_sync::{
    Client, Link, ObjectMirror, Op, Store, relay, run_link_stdio_to_unix_socket, run_server_on_unix_socket,
};

fn machine(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        added_at_millis: 1,
        ..Machine::default()
    }
}

#[test]
fn a_server_on_a_socket_answers_one_computer_and_removes_its_socket_when_it_stops() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sock");
    let store = Store::open(&temp.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&path, store.clone()).expect("the socket");
    assert_eq!(server.path(), path.as_path());
    assert!(
        fs::symlink_metadata(&path)
            .expect("the socket file")
            .file_type()
            .is_socket(),
        "the server binds one socket"
    );
    assert_eq!(
        fs::metadata(&path).expect("the socket file").permissions().mode() & 0o777,
        0o600,
        "the socket is the server user's alone"
    );

    let stream = UnixStream::connect(&path).expect("a link");
    let mut link = Link::new(stream.try_clone().expect("the read end"), stream);
    let mut client = Client::open(temp.path().join("queue.json"), "mbp-7c1").expect("a queue");
    let answer = client.hello(&mut link).expect("the hello");
    assert_eq!(answer.store, store.store_id());
    assert!(answer.now_millis > 0, "the server stamps its own time");

    client
        .enqueue(
            None,
            Op::MachinePut {
                record: machine("build-1"),
            },
        )
        .expect("the intent is queued");
    let outcome = client.push_pending(&mut link).expect("the push");
    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(store.index(), 1);

    server.stop();
    assert!(!path.exists(), "the socket file is gone with the server");
    assert!(UnixStream::connect(&path).is_err(), "and no server answers");
    store.close();
}

#[test]
fn the_relay_carries_the_frames_of_a_link_and_ends_when_its_stream_ends() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("relay.sock");
    let store = Store::open(&temp.path().join("server.sqlite")).expect("the store");
    let server = run_server_on_unix_socket(&path, store.clone()).expect("the socket");

    // The pair is the stdio of the child of a later slice: one end is the child,
    // and the relay copies its bytes to the socket of the server.
    let (child, parent) = UnixStream::pair().expect("a socket pair");
    let relaying = {
        let socket = path.clone();
        let reading = child.try_clone().expect("the read end of the child");
        thread::spawn(move || relay(reading, child, &socket))
    };

    let mut link = Link::new(parent.try_clone().expect("the read end"), parent);
    let mut client = Client::open(temp.path().join("queue.json"), "mbp-7c1").expect("a queue");
    let answer = client.hello(&mut link).expect("the hello over the relay");
    assert_eq!(answer.store, store.store_id());
    client
        .enqueue(
            None,
            Op::MachinePut {
                record: machine("build-1"),
            },
        )
        .expect("the intent is queued");
    client.push_pending(&mut link).expect("the push over the relay");
    let mut mirror = ObjectMirror::default();
    let pulled = client.pull(&mut link, &mut mirror).expect("the pull over the relay");
    assert_eq!(pulled.index, 1);
    assert_eq!(mirror.len(), 1);

    // The end of the child's stdin ends its side of the link.
    drop(link);
    drop(client);
    relaying.join().expect("the relay thread").expect("the relay ends");
    // The server answers one link at a time; give it the moment to end it.
    thread::sleep(Duration::from_millis(50));
    server.stop();
    store.close();
}

#[test]
fn a_path_of_the_person_that_is_not_a_socket_is_left_alone() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("occupied.sock");
    fs::write(&path, b"I am a note of the person").expect("a file");
    let store = Store::open(&temp.path().join("server.sqlite")).expect("the store");
    let refused = run_server_on_unix_socket(&path, store.clone());
    assert!(refused.is_err(), "a file of the person is not replaced by a socket");
    assert_eq!(
        fs::read(&path).expect("the file"),
        b"I am a note of the person",
        "the file is unchanged"
    );
    fs::remove_file(&path).expect("the note");

    // A socket that a stopped server left is removed and bound again.
    let stale = UnixListener::bind(&path).expect("a stale socket");
    drop(stale);
    assert!(
        fs::symlink_metadata(&path)
            .expect("the stale socket")
            .file_type()
            .is_socket()
    );
    let server = run_server_on_unix_socket(&path, store.clone()).expect("the socket is bound again");
    server.stop();
    store.close();
}

#[test]
fn the_stdio_link_reports_a_socket_that_is_not_there() {
    let temp = tempfile::tempdir().expect("a directory");
    let missing = temp.path().join("no-server.sock");
    let refused = run_link_stdio_to_unix_socket(&missing).expect_err("no server answers");
    assert!(
        refused.to_string().contains("no-server.sock"),
        "the refusal names the path: {refused}"
    );
}

/// A link whose standard input stays open must still end when the socket ends.
/// This is the state of an idle computer: nothing of this computer writes, the
/// server stops or dies, and the mode of the link must end instead of waiting for
/// a byte that never comes.
#[test]
fn a_relay_returns_within_a_second_when_the_socket_ends_while_the_input_stays_open() {
    let temp = tempfile::tempdir().expect("a directory");
    let path = temp.path().join("server.sock");
    let listener = UnixListener::bind(&path).expect("the socket listener");

    // The two halves of the mode of a link: the standard input that nothing
    // writes to, and the standard output that nothing reads. Both pipes stay open
    // for the whole test, so neither side ends on its own.
    let (requests, _requests_open) = std::io::pipe().expect("the input of the relay");
    let (_answers_open, answers) = std::io::pipe().expect("the output of the relay");
    let (returned, returning) = std::sync::mpsc::channel();
    let relaying = thread::spawn(move || {
        let result = relay(requests, answers, &path);
        let _ = returned.send(result.is_ok());
    });

    // The server side of the link connects and then goes, as a stopped server does.
    let (peer, _address) = listener.accept().expect("the link of the relay");
    drop(peer);
    assert_eq!(
        returning.recv_timeout(Duration::from_secs(1)),
        Ok(true),
        "the relay returned at once after the socket ended"
    );
    relaying.join().expect("the relay thread");
}
