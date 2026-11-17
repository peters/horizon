//! What the scanner costs on a stream that holds no status sequence at all.
//!
//! Almost every byte of a panel's output is text, so the cost of the tap is the
//! cost of looking at a stream without a report in it: one pass that stops at the
//! next escape byte. This compares that pass with the plain reader over the same
//! bytes, for plain output and for output with one escape byte in every 64, which
//! is the worst case of a program that draws colour or cursor movement.
//!
//! The same reader copies the output for a follower. Every pass above runs with
//! nobody following, so its bound is also the bound of that tap at rest; the last
//! test measures the copy itself.

use std::io::{self, Read};
use std::time::{Duration, Instant};

use alacritty_terminal::vte::ansi::{Handler, Processor, StdSyncHandler};

use crate::inbox::StatusTap;
use crate::reader::StatusReader;

/// The number of bytes of one pass: 64 MiB.
const BYTES: usize = 64 * 1024 * 1024;
/// The size of one read, which is the buffer of the event loop of a panel.
const READ: usize = 8 * 1024;

/// A stream of `chunks` repeated to a length, which allocates no output.
struct Repeating {
    chunk: Vec<u8>,
    left: usize,
}

impl Repeating {
    fn new(chunk: Vec<u8>) -> Self {
        Self { chunk, left: BYTES }
    }
}

impl Read for Repeating {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let take = buffer.len().min(self.left);
        if take == 0 {
            return Ok(0);
        }
        let mut written = 0;
        while written < take {
            let offset = written % self.chunk.len();
            let step = (take - written).min(self.chunk.len() - offset);
            buffer[written..written + step].copy_from_slice(&self.chunk[offset..offset + step]);
            written += step;
        }
        self.left -= take;
        Ok(take)
    }
}

/// The bytes of one pass: text only, or text with one escape byte in every 64.
fn chunk(with_escape: bool) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(64);
    for index in 0..64u8 {
        if with_escape && index == 0 {
            // A colour change of four bytes and then text of the rest.
            chunk.extend_from_slice(&[0x1b, b'[', b'3', b'2', b'm']);
        } else {
            chunk.push(b'a' + (index % 26));
        }
    }
    chunk
}

/// The bytes that a pass over `with_escape` reads, through a reader of `tap`.
fn pass(with_escape: bool, tap: bool) -> (Duration, usize) {
    let source = Repeating::new(chunk(with_escape));
    let mut buffer = vec![0u8; READ];
    let read = |reader: &mut dyn Read, buffer: &mut [u8]| -> usize {
        let mut total = 0;
        loop {
            match reader.read(buffer) {
                Ok(0) | Err(_) => return total,
                Ok(count) => total += count,
            }
        }
    };
    let started = Instant::now();
    let total = if tap {
        let (status_tap, _inbox) = StatusTap::new();
        let mut reader = StatusReader::new(source, status_tap);
        read(&mut reader, &mut buffer)
    } else {
        let mut reader = source;
        read(&mut reader, &mut buffer)
    };
    (started.elapsed(), total)
}

/// The cost of the tap over one pass, in whole percent of the plain pass.
fn cost_percent(with_escape: bool) -> (Duration, Duration, u128) {
    // A warm pass of each, so neither pays for the first touch of its pages.
    let _ = pass(with_escape, false);
    let _ = pass(with_escape, true);
    let (plain, plain_bytes) = least_of_five(with_escape, false);
    let (scanned, scanned_bytes) = least_of_five(with_escape, true);
    assert_eq!(plain_bytes, BYTES, "the plain pass read every byte");
    assert_eq!(scanned_bytes, BYTES, "the scanned pass read every byte");
    let percent = scanned.as_millis().saturating_mul(100) / plain.as_millis().max(1);
    (plain, scanned, percent)
}

/// The least of five passes over `with_escape` through the reader of `tap`.
///
/// The least time is the run that the rest of the computer disturbed least, so a
/// single loaded sample cannot decide the comparison of the two sides.
fn least_of_five(with_escape: bool, tap: bool) -> (Duration, usize) {
    let mut least = pass(with_escape, tap);
    for _ in 1..5 {
        let run = pass(with_escape, tap);
        least = (least.0.min(run.0), least.1);
    }
    least
}

#[test]
fn the_scan_of_plain_output_costs_a_small_part_of_the_read() {
    let (plain, scanned, percent) = cost_percent(false);
    eprintln!("64 MiB of text: plain {plain:?}, with the tap {scanned:?} ({percent} % of the plain pass)");
    assert!(
        scanned <= plain.saturating_mul(3).saturating_add(Duration::from_millis(250)),
        "the tap must stay near the plain pass: {scanned:?} against {plain:?}"
    );
}

#[test]
fn the_scan_of_output_with_escape_bytes_costs_a_small_part_of_the_read() {
    let (plain, scanned, percent) = cost_percent(true);
    eprintln!(
        "64 MiB with one escape byte in 64: plain {plain:?}, with the tap {scanned:?} ({percent} % of the plain pass)"
    );
    assert!(
        scanned <= plain.saturating_mul(3).saturating_add(Duration::from_millis(250)),
        "the tap must stay near the plain pass: {scanned:?} against {plain:?}"
    );
}

#[test]
fn the_scanned_pass_reads_the_same_bytes_as_the_plain_one() {
    let source = Repeating::new(chunk(true));
    let (status_tap, inbox) = StatusTap::new();
    let mut scanned = StatusReader::new(source, status_tap);
    let mut bytes = Vec::new();
    scanned.read_to_end(&mut bytes).expect("the pass reads to its end");

    let mut expected = Vec::new();
    Repeating::new(chunk(true))
        .take(BYTES as u64)
        .read_to_end(&mut expected)
        .expect("the plain pass reads to its end");

    assert_eq!(bytes.len(), expected.len());
    assert_eq!(bytes, expected, "the reader shows every byte unchanged");
    assert!(inbox.take_report().is_none(), "the stream holds no report");
}

/// A handler that keeps nothing: the parse of a stream measures the parser.
struct Noop;

impl Handler for Noop {}

/// The time that the VT parser of the panel takes for the same bytes.
fn parse(with_escape: bool) -> Duration {
    let source = Repeating::new(chunk(with_escape));
    let mut bytes = Vec::with_capacity(BYTES);
    source
        .take(BYTES as u64)
        .read_to_end(&mut bytes)
        .expect("the pass of the test");
    let mut processor: Processor<StdSyncHandler> = Processor::new();
    let mut handler = Noop;
    let started = Instant::now();
    processor.advance(&mut handler, &bytes);
    started.elapsed()
}

/// The scan of the text of a panel is one search per read, and it must cost less
/// than the read of the same bytes. Both bounds are measured on the same run, so
/// they hold on a slower computer too: about 0.4 ms are added to a read of 3.8 ms
/// and to a parse of 150 ms per 64 MiB of text.
#[test]
fn the_added_cost_of_plain_output_stays_far_below_its_read() {
    let (plain, scanned, _percent) = cost_percent(false);
    let parsed = parse(false);
    let added = scanned.saturating_sub(plain);
    eprintln!("64 MiB of text: added {added:?} to a read of {plain:?} and a parse of {parsed:?}");
    assert!(added <= plain, "the tap adds {added:?} to a read of {plain:?}");
    assert!(added <= parsed / 10, "the tap adds {added:?} to a parse of {parsed:?}");
}

#[test]
fn the_scan_costs_a_small_part_of_the_parse_of_the_same_bytes() {
    for (label, with_escape) in [("text", false), ("one escape byte in 64", true)] {
        let (plain, scanned, _percent) = cost_percent(with_escape);
        let parsed = parse(with_escape);
        eprintln!("64 MiB of {label}: read {plain:?}, read and scan {scanned:?}, parse {parsed:?}");
        assert!(
            scanned.saturating_sub(plain) <= parsed.saturating_add(Duration::from_millis(50)),
            "the scan of the same bytes must cost less than their parse: {scanned:?} against {parsed:?}"
        );
    }
}

/// The reads between two takes of the follower of a pass: 512 KiB, half the bound
/// of the outlet.
const TAKE_EVERY: usize = 64;

/// A pass over text through a reader whose follower `follows` or not: the time,
/// the bytes that the panel read and the bytes that the follower took.
fn shared_pass(follows: bool) -> (Duration, usize, usize) {
    let (status_tap, _inbox) = StatusTap::new();
    let mut reader = StatusReader::new(Repeating::new(chunk(false)), status_tap);
    let outlet = reader.share();
    outlet.set_wanted(follows);
    outlet.start();
    let mut buffer = vec![0u8; READ];
    let (mut total, mut taken, mut reads) = (0, 0, 0usize);
    let started = Instant::now();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => total += count,
        }
        reads += 1;
        if reads.is_multiple_of(TAKE_EVERY) {
            let take = outlet.take();
            assert!(!take.overflowed, "the follower of the pass keeps up");
            taken += take.bytes.len();
        }
    }
    taken += outlet.take().bytes.len();
    (started.elapsed(), total, taken)
}

/// The least of five passes of `shared_pass`, for the reason of `least_of_five`.
fn least_shared(follows: bool) -> (Duration, usize, usize) {
    let _ = shared_pass(follows);
    (0..5)
        .map(|_| shared_pass(follows))
        .min_by_key(|run| run.0)
        .unwrap_or_default()
}

/// A panel that nobody follows pays one atomic load for each read, so its pass
/// costs what the scan alone costs. A panel that somebody follows pays one copy
/// of its output, which must stay below the parse of the same bytes.
#[test]
fn the_copy_for_a_follower_costs_nothing_at_rest_and_less_than_the_parse_when_on() {
    let (scanned, _bytes) = least_of_five(false, true);
    let (resting, resting_bytes, resting_taken) = least_shared(false);
    let (copying, copying_bytes, copying_taken) = least_shared(true);
    let parsed = parse(false);
    eprintln!(
        "64 MiB of text: scan {scanned:?}, with a follower at rest {resting:?}, with a follower {copying:?}, parse {parsed:?}"
    );

    assert_eq!(resting_bytes, BYTES, "the panel read every byte");
    assert_eq!(resting_taken, 0, "no byte is copied while nobody follows");
    assert_eq!(copying_bytes, BYTES, "the panel read every byte");
    assert_eq!(copying_taken, BYTES, "the follower took every byte");
    assert!(
        resting <= scanned.saturating_mul(2).saturating_add(Duration::from_millis(250)),
        "the tap at rest must cost what the scan costs: {resting:?} against {scanned:?}"
    );
    assert!(
        copying.saturating_sub(resting) <= parsed.saturating_add(Duration::from_millis(50)),
        "the copy must cost less than the parse of the same bytes: {copying:?} against {parsed:?}"
    );
}
