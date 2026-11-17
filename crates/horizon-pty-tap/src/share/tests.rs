use std::io::{Cursor, Read};
use std::sync::mpsc;
use std::time::Duration;

use super::{SHARE_BUFFER_BYTES, ShareOutlet, ShareTake};
use crate::inbox::StatusTap;
use crate::reader::StatusReader;

/// The bound of a wait of a test, so a reader that waits on a lock fails the test
/// instead of hanging it.
const HANG_GUARD: Duration = Duration::from_secs(30);

/// A reader over `bytes` with the outlet of its follower.
fn reader(bytes: Vec<u8>) -> (StatusReader<Cursor<Vec<u8>>>, ShareOutlet) {
    let (tap, _inbox) = StatusTap::new();
    let reader = StatusReader::new(Cursor::new(bytes), tap);
    let outlet = reader.share();
    (reader, outlet)
}

/// Reads `source` to its end in reads of `step` bytes, as the event loop of a
/// panel reads its PTY.
fn read_all(source: &mut impl Read, step: usize) -> Vec<u8> {
    let mut seen = Vec::new();
    let mut buffer = vec![0u8; step];
    loop {
        let read = source.read(&mut buffer).expect("a read of the test");
        if read == 0 {
            return seen;
        }
        seen.extend_from_slice(&buffer[..read]);
    }
}

/// A stream of `length` bytes that no two neighbours share, so a lost, a doubled
/// or a moved byte shows.
fn stream(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or_default())
        .collect()
}

fn nothing(overflowed: bool) -> ShareTake {
    ShareTake {
        bytes: Vec::new(),
        overflowed,
    }
}

#[test]
fn no_byte_reaches_the_outlet_while_nobody_follows() {
    let (mut source, outlet) = reader(stream(4096));
    // A screen without a follower starts nothing.
    outlet.start();

    assert_eq!(
        read_all(&mut source, 512),
        stream(4096),
        "the panel still reads every byte"
    );
    assert_eq!(outlet.take(), nothing(false));
}

#[test]
fn a_follower_gets_nothing_before_a_screen_and_is_told_to_ask_for_one() {
    let (mut source, outlet) = reader(stream(4096));
    outlet.set_wanted(true);

    let _ = read_all(&mut source, 512);

    assert_eq!(
        outlet.take(),
        nothing(true),
        "output that follows no screen is not kept"
    );
    assert_eq!(outlet.take(), nothing(true), "and the outlet says so until a screen");
}

#[test]
fn the_outlet_gives_the_bytes_after_a_screen_in_the_order_of_the_program() {
    let (mut source, outlet) = reader(stream(10_000));
    outlet.set_wanted(true);
    // The follower of the test asks twice: the second ask changes nothing.
    outlet.set_wanted(true);
    let mut before = vec![0u8; 1000];
    source.read_exact(&mut before).expect("the output before the screen");
    outlet.start();

    let mut taken = Vec::new();
    let mut buffer = vec![0u8; 700];
    loop {
        let read = source.read(&mut buffer).expect("a read of the test");
        if read == 0 {
            break;
        }
        let take = outlet.take();
        assert!(!take.overflowed);
        taken.extend_from_slice(&take.bytes);
    }

    assert_eq!(
        taken,
        stream(10_000)[1000..],
        "every byte after the screen, once and in order"
    );
    assert_eq!(outlet.take(), nothing(false), "a take leaves nothing behind");
}

#[test]
fn more_than_the_bound_empties_the_outlet_until_the_next_screen() {
    let (mut source, outlet) = reader(stream(SHARE_BUFFER_BYTES + 3 * 4096));
    outlet.set_wanted(true);
    outlet.start();
    let mut exact = vec![0u8; SHARE_BUFFER_BYTES];
    source.read_exact(&mut exact).expect("the bound of the outlet");
    // Exactly the bound still waits whole. The take of the test looks at the
    // length without a swap, because a swap would empty the outlet.
    assert_eq!(outlet.shared.state().bytes.len(), SHARE_BUFFER_BYTES);

    let mut one = [0u8; 1];
    source.read_exact(&mut one).expect("the byte over the bound");
    assert_eq!(outlet.take(), nothing(true), "what waited is dropped whole");

    let mut more = [0u8; 4096];
    source.read_exact(&mut more).expect("output after the overflow");
    assert_eq!(outlet.take(), nothing(true), "and nothing is kept until a screen");
    assert_eq!(
        outlet.shared.state().bytes.capacity(),
        0,
        "the outlet gave its memory back"
    );

    outlet.start();
    let rest = read_all(&mut source, 4096);
    assert_eq!(
        outlet.take(),
        ShareTake {
            bytes: rest,
            overflowed: false
        },
        "a new screen starts the outlet again"
    );
}

#[test]
fn a_follower_that_leaves_empties_the_outlet() {
    let (mut source, outlet) = reader(stream(8192));
    outlet.set_wanted(true);
    outlet.start();
    let mut first = [0u8; 4096];
    source.read_exact(&mut first).expect("output for the follower");

    outlet.set_wanted(false);
    assert_eq!(
        outlet.shared.state().bytes.capacity(),
        0,
        "the outlet gave its memory back"
    );
    let _ = read_all(&mut source, 512);

    assert_eq!(outlet.take(), nothing(false));
    outlet.set_wanted(true);
    assert_eq!(outlet.take(), nothing(true), "a new follower needs a screen first");
}

#[test]
fn a_read_takes_no_lock_while_the_outlet_copies_nothing() {
    for wanted in [false, true] {
        let (mut source, outlet) = reader(stream(64 * 1024));
        // A follower without a screen copies nothing either.
        outlet.set_wanted(wanted);
        let (done_tx, done_rx) = mpsc::channel();
        let held = outlet.shared.state();
        let reads = std::thread::spawn(move || {
            let seen = read_all(&mut source, 1024);
            let _ = done_tx.send(seen.len());
        });

        // The lock of the outlet is held here for the whole pass, so a read that
        // took it would never end.
        let read = done_rx.recv_timeout(HANG_GUARD);
        drop(held);
        reads.join().expect("the reads of the test");

        assert_eq!(read, Ok(64 * 1024), "the reads ended while the lock was held");
        assert!(outlet.take().bytes.is_empty(), "and no byte was copied");
    }
}

#[test]
fn a_take_on_another_thread_loses_no_byte_and_keeps_the_order() {
    const LENGTH: usize = 4 * 1024 * 1024;
    let (mut source, outlet) = reader(stream(LENGTH));
    let (done_tx, done_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let follower = outlet.clone();
    // The screen and the takes that continue it belong to one thread, so the
    // taker of the test starts the screen of the stream before it reads.
    let taking = std::thread::spawn(move || {
        follower.set_wanted(true);
        follower.start();
        started_tx.send(()).expect("the screen of the taker of the test");
        let mut taken = Vec::new();
        let mut done = false;
        // The last take runs after the reader ended, so it holds the tail.
        while !done {
            done = done_rx.try_recv().is_ok();
            let take = follower.take();
            assert!(!take.overflowed, "the taker of the test keeps up");
            taken.extend_from_slice(&take.bytes);
        }
        taken
    });

    started_rx
        .recv()
        .expect("the screen of the taker, before the first read");
    // The reader waits for the taker when the outlet is nearly full, as a slow
    // program would: the test proves the order and not the overflow rule.
    let mut buffer = vec![0u8; 8 * 1024];
    loop {
        while outlet.shared.state().bytes.len() > SHARE_BUFFER_BYTES / 2 {
            std::thread::yield_now();
        }
        if source.read(&mut buffer).expect("a read of the test") == 0 {
            break;
        }
    }
    done_tx.send(()).expect("the taker of the test");
    let taken = taking.join().expect("the taker of the test");

    assert_eq!(taken.len(), LENGTH);
    assert!(taken == stream(LENGTH), "the takes hold the stream once and in order");
}

#[cfg(debug_assertions)]
#[test]
fn a_take_on_a_thread_that_started_no_screen_is_reported() {
    let (_source, outlet) = reader(stream(64));
    outlet.set_wanted(true);
    outlet.start();
    let other = outlet.clone();
    let reported =
        std::thread::spawn(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| other.take())).is_err())
            .join()
            .expect("the second thread of the test");

    assert!(
        reported,
        "a take on another thread than the screen of the outlet is a panic of a debug build"
    );
    assert_eq!(
        outlet.take(),
        nothing(false),
        "the thread of the screen still takes what follows it"
    );
}
