//! The link of a holder as bytes that are untrusted input: a frame that arrives
//! in parts, a frame above the bound of a payload, a peer that ends inside a
//! frame, a write that the link takes in parts and a read with no answer.

use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::ChildEvent;

use super::{LinkReader, LinkWriter};
use crate::frames::{self, Frame, Frames};

/// The longest time that one test waits for the other side of its link.
const HANG_GUARD: Duration = Duration::from_secs(30);

/// A reader over the link, and the socket of its peer, which is the holder.
fn reader() -> (LinkReader, UnixStream) {
    let (mine, peer) = UnixStream::pair().expect("a socket pair of the test");
    mine.set_nonblocking(true).expect("a non-blocking link");
    (LinkReader::new(mine), peer)
}

/// A writer over the link, and the socket of its peer, which is the holder.
fn writer() -> (LinkWriter, UnixStream) {
    let (mine, peer) = UnixStream::pair().expect("a socket pair of the test");
    mine.set_nonblocking(true).expect("a non-blocking link");
    (LinkWriter::of(mine), peer)
}

/// The bytes of one frame, as the holder writes it.
fn encoded(frame: &Frame) -> Vec<u8> {
    let mut bytes = Vec::new();
    frames::write(&mut bytes, frame).expect("the bytes of a frame of the test");
    bytes
}

/// The one read of `link`: the bytes that it gives, or the reason that it gives
/// none.
fn read(link: &mut LinkReader) -> std::io::Result<Vec<u8>> {
    let mut buffer = [0u8; 4096];
    let got = link.read(&mut buffer)?;
    Ok(buffer[..got].to_vec())
}

/// The frames that the peer of a writer holds now, read until nothing follows.
fn frames_of(peer: &mut UnixStream) -> Vec<Frame> {
    peer.set_nonblocking(true)
        .expect("a peer of a test that waits for nothing");
    let mut whole = Frames::default();
    let mut answered = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => return answered,
            Ok(got) => answered.extend(whole.push(&buffer[..got]).expect("the frames of the link")),
            Err(error) if error.kind() == ErrorKind::WouldBlock => return answered,
            Err(error) => panic!("the peer of the test reads no frame of the link: {error}"),
        }
    }
}

#[test]
fn a_frame_that_the_link_takes_in_both_of_its_parts_gives_its_bytes_once() {
    let (mut link, mut peer) = reader();
    let frame = encoded(&Frame::Output(b"the output of the program".to_vec()));
    let cut = frames::HEADER + 4;
    peer.write_all(&frame[..cut]).expect("the first part of a frame");

    assert_eq!(
        read(&mut link).expect_err("half a frame is no output").kind(),
        ErrorKind::WouldBlock,
        "a cut frame gives no byte of its payload"
    );

    peer.write_all(&frame[cut..]).expect("the second part of the frame");
    assert_eq!(
        read(&mut link).expect("the bytes of the whole frame"),
        b"the output of the program",
        "the payload of a frame that arrived in two parts reaches the terminal once"
    );
    assert_eq!(
        read(&mut link).expect_err("no second copy of those bytes").kind(),
        ErrorKind::WouldBlock,
        "the bytes of a frame reach the terminal once"
    );
}

#[test]
fn a_frame_above_the_bound_of_a_payload_is_refused_and_no_byte_of_it_reaches_the_terminal() {
    let (mut link, mut peer) = reader();
    let above = usize::try_from(frames::MAX_PAYLOAD).unwrap_or(usize::MAX) + 1;
    let kind = Frame::Output(Vec::new()).kind();
    peer.write_all(&frames::header(kind, above))
        .expect("the header of a frame above the bound");
    peer.write_all(b"bytes of a frame that no holder may send")
        .expect("the payload of that frame");

    let error = read(&mut link).expect_err("no frame of that length");
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("above the bound"),
        "the error names the bound of a payload: {error}"
    );
}

#[test]
fn the_peer_that_ends_inside_a_frame_ends_the_link_without_its_bytes() {
    let (mut link, mut peer) = reader();
    let frame = encoded(&Frame::Output(b"bytes that no whole frame ends".to_vec()));
    peer.write_all(&frame[..frames::HEADER + 3])
        .expect("half a frame of the peer");
    peer.shutdown(Shutdown::Write).expect("the end of the peer");

    assert_eq!(
        read(&mut link).expect("the end of a link reports no byte"),
        Vec::<u8>::new(),
        "the part of a frame that arrived reaches no terminal"
    );
    assert_eq!(
        link.ended_child(),
        Some(ChildEvent::Exited(None)),
        "a link that ended inside a frame is the end of the child of the terminal"
    );
}

#[test]
fn a_read_that_the_link_has_no_answer_for_waits_and_ends_no_child() {
    let (mut link, mut peer) = reader();

    assert_eq!(
        read(&mut link).expect_err("no byte of a link that gave none").kind(),
        ErrorKind::WouldBlock,
        "a link with nothing to read waits instead of ending"
    );
    assert_eq!(link.ended_child(), None, "a link that gave no byte has not ended");

    peer.write_all(&encoded(&Frame::Output(b"one".to_vec())))
        .expect("output of the peer");
    assert_eq!(
        link.ended_child(),
        None,
        "a link with a byte to read is a link that still runs"
    );
    assert_eq!(read(&mut link).expect("the bytes of the peer"), b"one");
}

#[test]
fn the_exit_frame_of_the_holder_ends_the_child_after_the_output_before_it() {
    let (mut link, mut peer) = reader();
    let mut bytes = encoded(&Frame::Output(b"last output".to_vec()));
    bytes.extend_from_slice(&encoded(&Frame::Exit(Some(3))));
    peer.write_all(&bytes).expect("the end of the program of the peer");

    assert_eq!(read(&mut link).expect("the output before the end"), b"last output");
    assert_eq!(
        read(&mut link).expect("the end of the link reports no byte"),
        Vec::<u8>::new()
    );
    assert_eq!(
        link.ended_child(),
        Some(ChildEvent::Exited(Some(ExitStatus::from_raw(3 << 8)))),
        "the status of the exit frame is the status of the child"
    );
}

#[test]
fn the_refusal_of_a_holder_ends_the_link_of_a_terminal_that_may_not_have_it() {
    let (mut link, mut peer) = reader();
    peer.write_all(&encoded(&Frame::Refusal))
        .expect("the refusal of the peer");

    let error = read(&mut link).expect_err("a refused link gives no byte");
    assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
}

#[test]
fn a_frame_that_no_holder_sends_to_a_terminal_is_refused() {
    let (mut link, mut peer) = reader();
    let close = encoded(&Frame::Close);
    peer.write_all(&close).expect("a frame that only a client sends");

    let error = read(&mut link).expect_err("a close frame is no output of a holder");
    assert_eq!(error.kind(), ErrorKind::Other);
    assert!(
        error.to_string().contains(&Frame::Close.kind().to_string()),
        "the error names the kind of the frame: {error}"
    );
}

#[test]
fn the_sizes_of_the_writer_reach_the_holder_in_the_order_of_its_calls() {
    let (mut link, mut peer) = writer();
    let attach = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 8,
        cell_height: 17,
    };
    link.resize(attach);
    link.resize(WindowSize {
        num_lines: 40,
        num_cols: 120,
        ..attach
    });

    assert_eq!(
        frames_of(&mut peer),
        vec![
            Frame::Resize {
                rows: 24,
                cols: 80,
                cell_width: 8,
                cell_height: 17,
            },
            Frame::Resize {
                rows: 40,
                cols: 120,
                cell_width: 8,
                cell_height: 17,
            },
        ],
        "a size reaches the holder as a size frame, and no input byte answers for it"
    );
    assert_eq!(link.write(b"hi").expect("the input of the terminal"), 2);
    assert_eq!(
        frames_of(&mut peer),
        vec![Frame::Input(b"hi".to_vec())],
        "the input follows the sizes of the terminal"
    );
}

#[test]
fn an_input_that_the_link_takes_in_parts_answers_every_byte_once() {
    let (mut link, mut peer) = writer();
    // More than one frame holds, so the link takes the input in frames of its own.
    let above = usize::try_from(frames::MAX_PAYLOAD).unwrap_or_default() + 4096;
    let payload: Vec<u8> = (0..above)
        .map(|at| u8::try_from(at % 251).unwrap_or_default())
        .collect();
    let wanted = payload.clone();
    let holder = thread::spawn(move || {
        let mut whole = Frames::default();
        let mut got = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        while got.len() < wanted.len() {
            let read = peer.read(&mut buffer).expect("a read of the holder of the test");
            assert!(read > 0, "the holder of the test reads the frames of the link");
            for frame in whole.push(&buffer[..read]).expect("the frames of the link") {
                match frame {
                    Frame::Input(bytes) => got.extend_from_slice(&bytes),
                    other => panic!("the writer sent the frame {}", other.kind()),
                }
            }
        }
        got
    });

    let deadline = Instant::now() + HANG_GUARD;
    let mut answers = Vec::new();
    let mut accepted = 0;
    while accepted < payload.len() {
        let took = link.write(&payload[accepted..]).expect("a write of the link");
        answers.push(took);
        accepted += took;
        assert!(Instant::now() < deadline, "the link takes the input of the test");
    }

    assert!(
        answers.len() > 1,
        "one frame holds at most MAX_PAYLOAD bytes, so the link answers the input in parts: {answers:?}"
    );
    assert_eq!(
        accepted,
        payload.len(),
        "the answers of the link add up to the input that it took"
    );
    assert_eq!(
        holder.join().expect("the holder of the test"),
        payload,
        "every input byte reaches the holder once, in the order of the terminal"
    );
}
