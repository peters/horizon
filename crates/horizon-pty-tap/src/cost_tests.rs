//! What the scanner costs on a stream that holds no status sequence at all.
//!
//! Almost every byte of a panel's output is text, so the cost of the tap is the
//! cost of looking at a stream without a report in it: one pass that stops at the
//! next escape byte. This compares that pass with the plain reader over the same
//! bytes, for plain output and for output with one escape byte in every 64, which
//! is the worst case of a program that draws colour or cursor movement.

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
    let (plain, plain_bytes) = pass(with_escape, false);
    let (scanned, scanned_bytes) = pass(with_escape, true);
    assert_eq!(plain_bytes, BYTES, "the plain pass read every byte");
    assert_eq!(scanned_bytes, BYTES, "the scanned pass read every byte");
    let percent = scanned.as_millis().saturating_mul(100) / plain.as_millis().max(1);
    (plain, scanned, percent)
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

#[test]
fn the_scan_costs_a_small_part_of_the_parse_of_the_same_bytes() {
    for (label, with_escape) in [("text", false), ("one escape byte in 64", true)] {
        let (plain, scanned, _percent) = cost_percent(with_escape);
        let parsed = parse(with_escape);
        eprintln!("64 MiB of {label}: read {plain:?}, read and scan {scanned:?}, parse {parsed:?}");
        assert!(
            scanned.saturating_sub(plain) <= parsed,
            "the scan of the same bytes must cost less than their parse: {scanned:?} against {parsed:?}"
        );
    }
}
