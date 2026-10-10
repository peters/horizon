use super::every_local;
use horizon_core::cloud_runtime::session_status::{SessionActivity, SessionStatus};
use std::collections::HashMap;

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
    assert!(none.iter().all(|(_, status)| status.activity == SessionActivity::Missing));
}
