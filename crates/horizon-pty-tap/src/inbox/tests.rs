use super::{QUEUE_DEPTH, StatusTap};
use crate::scanner::Payload;

/// One report of the test, named by its last byte.
fn report(marker: u8) -> Payload {
    Payload::for_tests(&[b'a', marker])
}

#[test]
fn the_newest_report_wins_and_an_older_one_is_not_read_later() {
    let (tap, inbox) = StatusTap::new();
    for marker in 1..=3 {
        tap.report(report(marker));
    }

    let newest = inbox.take_report().expect("the newest report");

    assert_eq!(newest.as_bytes(), b"a\x03");
    assert!(inbox.take_report().is_none(), "no older report waits for a later frame");
}

#[test]
fn a_queue_that_is_full_drops_the_oldest_report() {
    let (tap, inbox) = StatusTap::new();
    let last = u8::try_from(QUEUE_DEPTH).unwrap_or(u8::MAX).saturating_add(3);
    for marker in 1..=last {
        tap.report(report(marker));
    }

    let newest = inbox.take_report().expect("the newest report");

    assert_eq!(newest.as_bytes(), [b'a', last].as_slice());
    assert!(inbox.take_report().is_none());
}

#[test]
fn a_query_waits_for_one_frame_and_is_taken_once() {
    let (tap, inbox) = StatusTap::new();
    tap.report(Payload::for_tests(super::QUERY_PAYLOAD));

    assert!(inbox.take_report().is_none(), "the query is no report of a state");
    assert!(inbox.take_query());
    assert!(!inbox.take_query(), "the query is answered once");
}
