use super::{Parking, every_local};
use horizon_core::cloud_runtime::session_status::{SessionActivity, SessionStatus};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn status(id: &str, activity: SessionActivity) -> SessionStatus {
    SessionStatus {
        id: id.into(),
        activity,
        quiet_for: None,
        lines: Vec::new(),
    }
}

#[test]
fn every_parked_panel_gets_a_status_and_an_unreported_one_is_missing() {
    let tmux = HashMap::from([
        ("tmux-1".to_owned(), "one".to_owned()),
        ("tmux-2".to_owned(), "two".to_owned()),
    ]);
    let locals = ["one".to_owned(), "two".to_owned(), "unrecorded".to_owned()];
    // The worker reports only the first session, and one that is not parked.
    let read = every_local(
        &locals,
        &tmux,
        vec![
            status("tmux-1", SessionActivity::Idle),
            status("tmux-9", SessionActivity::Working),
        ],
    );

    let activity: Vec<(&str, &str, SessionActivity)> = read
        .iter()
        .map(|(local, status)| (local.as_str(), status.id.as_str(), status.activity))
        .collect();
    assert_eq!(
        activity,
        [
            ("one", "tmux-1", SessionActivity::Idle),
            ("two", "tmux-2", SessionActivity::Missing),
            ("unrecorded", "", SessionActivity::Missing),
        ]
    );
    // Without any recorded session, each parked panel is still missing.
    let none = every_local(&locals, &HashMap::new(), Vec::new());
    assert!(
        none.iter()
            .all(|(_, status)| status.activity == SessionActivity::Missing)
    );
}

#[test]
fn a_read_that_misses_a_newly_parked_panel_does_not_count_as_new() {
    let mut parking = Parking::default();
    let (started, now) = (Instant::now(), Instant::now() + Duration::from_secs(2));
    parking.read_started = Some(started);
    let read = vec![("one".to_owned(), status("tmux-1", SessionActivity::Idle))];
    let parked = ["one".to_owned(), "new".to_owned()];

    parking.apply_read(Ok(read.clone()), &parked, now);
    assert!(!parking.read_since(started), "the new panel has no status yet");
    assert_eq!(parking.next_read, Some(now), "the next read is due at once");
    assert_eq!(parking.statuses["one"].activity, SessionActivity::Idle);

    parking.apply_read(Ok(read), &parked[..1], now);
    assert!(parking.read_since(started));
}
