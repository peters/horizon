//! The end of a holder that no panel shows, for a program that wrote much.
//!
//! The ring of a real agent is larger than a socket holds, so the client that
//! ends a holder must take its replay, and the holder must take its `Close`.

use std::collections::BTreeSet;

use super::super::serve::RING_LIMIT;
use super::{Client, Frame, Serving, attach_when_free, launch, temp_home};

/// The ids of real panels: 36 characters, as the app gives them.
const WROTE_MUCH: &str = "5a0f3c2e-8b1d-4e7a-9c64-0d2e7b1f8a35";
const NOT_SHOWN: &str = "c1d7e4b9-2f60-4a8e-b3a5-6e9f0a7d4c12";

/// The script of a program that writes `bytes` of text, then one marker, and waits.
fn writes(bytes: usize) -> String {
    format!("head -c {bytes} /dev/zero | tr '\\0' x; printf 'the last line\\n'; exec cat")
}

/// Starts a holder whose ring holds `bytes` of text and that no client reads.
fn holder_with_a_ring_of(home: &crate::HorizonHome, bytes: usize) -> (Serving, u32) {
    let serving = Serving::start(home, &launch(WROTE_MUCH, &writes(bytes)), RING_LIMIT);
    let (mut client, hello) = attach_when_free(&serving.socket);
    let (mut seen, mut last) = (0, Vec::new());
    while seen < bytes || !last.ends_with(b"the last line\r\n") {
        match client.next() {
            Frame::Output(part) | Frame::Replay(part) => {
                seen += part.len();
                last.extend_from_slice(&part);
                last.drain(..last.len().saturating_sub(64));
            }
            other => panic!("the holder sent {other:?}"),
        }
    }
    drop(client);
    (serving, hello.pid)
}

/// The start of a board that does not show the panel ends its holder: the program
/// is gone and the socket is removed.
fn a_start_ends_the_holder_of_a_ring_of(bytes: usize) {
    let (_folder, home) = temp_home();
    let (serving, pid) = holder_with_a_ring_of(&home, bytes);
    let socket = serving.socket.clone();

    let ends =
        super::super::close_unshown(&home, &BTreeSet::from([NOT_SHOWN.to_owned()])).expect("the holders of the home");
    assert_eq!((ends.ended, ends.alive), (1, 0), "the start ended the one holder");
    assert!(
        !crate::process::is_alive(pid),
        "the program is gone when the start answers"
    );
    assert!(!socket.exists(), "the socket is gone when the start answers");
    serving.ended();
}

/// A ring above the buffer of a socket, as the real agent had (350 KB).
#[test]
fn a_start_ends_an_unshown_holder_whose_ring_is_above_a_socket_buffer() {
    a_start_ends_the_holder_of_a_ring_of(512 * 1024);
}

/// A ring above the bound of one frame.
#[test]
fn a_start_ends_an_unshown_holder_whose_ring_is_above_one_frame() {
    a_start_ends_the_holder_of_a_ring_of(1024 * 1024 + 4096);
}

/// A terminal attaches to a holder whose ring is above the bound of one frame,
/// and it reads the last line of the program.
#[test]
fn a_link_takes_a_replay_above_one_frame() {
    let (_folder, home) = temp_home();
    let (serving, pid) = holder_with_a_ring_of(&home, 1024 * 1024 + 4096);

    let linked = super::super::linked(&home, WROTE_MUCH)
        .expect("the link of the holder")
        .expect("a holder that listens");
    assert_eq!(linked.hello.pid, pid);
    assert_eq!(linked.replay.len(), linked.hello.ring_len as usize);
    assert!(linked.replay.ends_with(b"the last line\r\n"));
    drop(linked);

    let ends = super::super::close_unshown(&home, &BTreeSet::new()).expect("the holders of the home");
    assert_eq!((ends.ended, ends.alive), (1, 0), "the holder ends after the link");
    serving.ended();
}

/// A client that sends `Close` and reads nothing still ends the program: the close
/// is the one frame that a holder never loses.
#[test]
fn a_close_of_a_client_that_takes_no_replay_ends_the_program() {
    let (_folder, home) = temp_home();
    let (serving, pid) = holder_with_a_ring_of(&home, 512 * 1024);

    let (mut client, _hello) = attach_when_free(&serving.socket);
    client.send(&Frame::Close);
    // The holder waits for this client twice its write bound, then it detaches it.
    let result = serving.result.recv_timeout(3 * super::TEST_BOUND);
    assert!(matches!(result, Ok(Ok(()))), "the holder ended with {result:?}");
    assert!(!crate::process::is_alive(pid), "the program ended");
    drop::<Client>(client);
}

/// A holder that does not answer is alive, and the start keeps its socket: a
/// socket that is removed names a holder that nothing can find again.
#[test]
fn a_start_keeps_the_socket_of_a_holder_that_answers_no_hello() {
    let (_folder, home) = temp_home();
    let paths = super::Paths::of(&home, WROTE_MUCH).expect("the socket of a holder");
    paths.prepare().expect("the folder of the sockets");
    // This listener takes a link and says nothing, as a holder whose loop is behind.
    let listener = paths.bind().expect("the socket of a holder of a test");

    let ends = super::super::close_unshown(&home, &BTreeSet::new()).expect("the holders of the home");
    assert_eq!(
        (ends.ended, ends.alive),
        (0, 1),
        "a holder that answers nothing is alive"
    );
    assert!(paths.socket().exists(), "the socket of a live holder stays");
    drop(listener);
}
