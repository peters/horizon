use super::{MAX_PAYLOAD_BYTES, Scanner};
use crate::inbox::StatusTap;

/// The payloads that `stream` reports when it arrives in `chunks` reads.
///
/// The hand-over of a panel is a level, so a frame reads the newest report of the
/// read in progress. This drains after every read, which is what a frame of the
/// test does, and it is also the only way to see more than one report of a stream.
fn payloads(stream: &[u8], chunks: usize) -> (Vec<String>, bool) {
    let step = stream.len().div_ceil(chunks.max(1)).max(1);
    let (tap, inbox) = StatusTap::new();
    let mut scanner = Scanner::new(tap);
    let mut found = Vec::new();
    let mut query = false;
    for chunk in stream.chunks(step) {
        scanner.feed(chunk);
        while let Some(payload) = inbox.take_report() {
            found.push(String::from_utf8_lossy(payload.as_bytes()).into_owned());
        }
        query |= inbox.take_query();
    }
    (found, query)
}

/// The stream that the probe of the real program recorded, in the byte forms that
/// it reported: the reports of one turn, a title before and after them, text
/// between them, and both terminators.
fn recorded_stream() -> Vec<u8> {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]0;work\x07");
    stream.extend_from_slice(b"\x1b]7501;state=idle:app=codex\x1b\\");
    stream.extend_from_slice(b"$ codex\r\n");
    stream.extend_from_slice(b"\x1b]0;\xe2\xa0\x8b work\x07");
    stream.extend_from_slice(b"\x1b]7501;state=working:app=codex\x1b\\");
    stream.extend_from_slice(b"\x1b]7501;state=blocked:app=codex:kind=permission\x1b\\");
    stream.extend_from_slice(b"\x1b]7501;state=working:app=codex\x1b\\");
    stream.extend_from_slice(b"\x1b]0;[ . ] Action Required | Create a file | work\x07");
    stream.extend_from_slice(b"Created `probe.txt`\r\n");
    stream.extend_from_slice(b"\x1b]7501;state=done:app=codex\x1b\\");
    stream.extend_from_slice(b"\x1b]7501;state=idle:app=codex\x1b\\");
    stream.extend_from_slice(b"\x1b]0;work\x07");
    stream.extend_from_slice(b"\x1b]7501;state=clear\x1b\\");
    stream
}

const RECORDED_PAYLOADS: [&str; 7] = [
    "state=idle:app=codex",
    "state=working:app=codex",
    "state=blocked:app=codex:kind=permission",
    "state=working:app=codex",
    "state=done:app=codex",
    "state=idle:app=codex",
    "state=clear",
];

#[test]
fn the_reports_of_a_turn_arrive_in_the_order_of_the_program() {
    // A read per byte, so every report has its own frame to arrive in: the
    // hand-over of a panel is a level, so two reports of one read leave the
    // newest one, as the record of the protocol does.
    let stream = recorded_stream();
    let (found, query) = payloads(&stream, stream.len());

    assert_eq!(found, RECORDED_PAYLOADS);
    assert!(!query, "the stream holds no feature detection query");
}

#[test]
fn a_read_that_holds_several_reports_hands_on_the_newest_one() {
    let (found, _query) = payloads(&recorded_stream(), 1);

    assert_eq!(found, ["state=clear"], "the newest report is the state of the record");
}

#[test]
fn a_sequence_split_at_any_byte_is_read_whole() {
    // One report between a title and the text that follows it, split at every
    // byte of the stream: no split may lose it or change a byte of it.
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]0;\xe2\xa0\x8b work\x07");
    stream.extend_from_slice(b"text before\r\n");
    stream.extend_from_slice(b"\x1b]7501;state=blocked:app=codex:kind=permission\x1b\\");
    stream.extend_from_slice(b"text after\r\n");
    for split in 0..=stream.len() {
        let (before, after) = stream.split_at(split);
        let (tap, inbox) = StatusTap::new();
        let mut scanner = Scanner::new(tap);
        let mut found = Vec::new();
        for part in [before, after] {
            scanner.feed(part);
            if let Some(payload) = inbox.take_report() {
                found.push(String::from_utf8_lossy(payload.as_bytes()).into_owned());
            }
        }
        assert_eq!(
            found,
            ["state=blocked:app=codex:kind=permission"],
            "the stream split at byte {split}"
        );
    }
}

#[test]
fn a_payload_beyond_the_bound_is_dropped_whole() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]7501;state=working:app=codex:pad=");
    stream.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD_BYTES));
    stream.extend_from_slice(b"\x1b\\");
    stream.extend_from_slice(b"after\r\n");
    stream.extend_from_slice(b"\x1b]7501;state=done:app=codex\x1b\\");

    let (found, _query) = payloads(&stream, 3);

    assert_eq!(
        found,
        ["state=done:app=codex"],
        "a report beyond the bound is dropped whole"
    );
}

#[test]
fn the_longest_payload_that_the_bound_allows_is_kept() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]7501;");
    let payload = vec![b'a'; MAX_PAYLOAD_BYTES];
    stream.extend_from_slice(&payload);
    stream.extend_from_slice(b"\x1b\\");

    let (found, _query) = payloads(&stream, 1);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].len(), MAX_PAYLOAD_BYTES);
}

#[test]
fn bytes_that_are_not_utf8_change_no_report_and_no_other_sequence() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&[0xff, 0xfe, 0x1b, b'[', b'3', b'1', b'm']);
    stream.extend_from_slice(b"\x1b]7501;state=working:app=codex:title=\xff\xfe\x1b\\");
    stream.extend_from_slice(&[0x80, 0x81]);

    let (found, _query) = payloads(&stream, 2);

    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("state=working:app=codex:title="));
    assert!(found[0].ends_with('\u{fffd}'), "the bytes arrive as they were written");
}

#[test]
fn another_osc_number_before_and_after_changes_nothing() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]0;Ctrl+C\x1b\\");
    stream.extend_from_slice(b"\x1b]52;c;aGVsbG8=\x07");
    stream.extend_from_slice(b"\x1b]7501;state=done:app=codex\x1b\\");
    stream.extend_from_slice(b"\x1b]10;?\x07");

    let (found, _query) = payloads(&stream, 2);

    assert_eq!(found, ["state=done:app=codex"]);
}

#[test]
fn the_feature_detection_query_is_not_a_report() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]7501;?\x1b\\");
    stream.extend_from_slice(b"\x1b]7501;state=working:app=codex\x1b\\");

    let (found, query) = payloads(&stream, 1);

    assert!(query, "the query reached the inbox as a query");
    assert_eq!(found, ["state=working:app=codex"]);
}

#[test]
fn a_sequence_without_its_end_reports_nothing() {
    let (found, _query) = payloads(b"\x1b]7501;state=wor", 1);

    assert!(found.is_empty());
}

#[test]
fn a_title_alone_reports_nothing() {
    let (found, _query) = payloads(b"\x1b]0;\xe2\xa0\x8b work\x07text\r\n", 1);

    assert!(found.is_empty());
}
