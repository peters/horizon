//! The mirror of the viewer: the frames that the pump reads, the rows of a
//! conversation, and the empty state of a start.

use horizon_core::agent_bridge::Entry;
use horizon_sync::{LiveEvent, MirrorComputer, MirrorEntry, MirrorProgram, MirrorSession};

use super::{Mirror, MirrorKey, MirrorView, SessionMirror};

fn session(index: u32) -> MirrorSession {
    MirrorSession {
        program: MirrorProgram::Claude,
        session: format!("00000000-0000-0000-0000-{index:012}"),
        title: format!("made-up session {index}"),
        folder: "repo".to_owned(),
        parent: "~/github".to_owned(),
        updated_at: 1_700_000_000_000,
        live: false,
    }
}

fn computer(sessions: Vec<MirrorSession>) -> MirrorComputer {
    MirrorComputer {
        device: "laptop-a".to_owned(),
        label: "Laptop A".to_owned(),
        sessions,
        unreadable: vec![MirrorProgram::Antigravity],
        own: false,
    }
}

fn key(index: u32) -> MirrorKey {
    MirrorKey {
        device: "laptop-a".to_owned(),
        program: MirrorProgram::Claude,
        session: session(index).session,
    }
}

fn entries(index: u32, texts: &[&str]) -> LiveEvent {
    LiveEvent::MirrorEntries {
        owner: "laptop-a".to_owned(),
        program: "claude".to_owned(),
        session: session(index).session,
        entries: texts
            .iter()
            .map(|text| MirrorEntry::Person {
                text: (*text).to_owned(),
            })
            .collect(),
        tail: true,
    }
}

#[test]
fn a_start_holds_nothing_of_a_mirror() {
    let view = MirrorView::default();
    assert!(view.is_empty());
    assert!(view.entries(&key(1)).is_empty());
    assert!(view.session(&key(1)).is_none());
    assert!(!SessionMirror::default().is_expanded("laptop-a"));
}

#[test]
fn the_rows_of_a_followed_session_are_the_rows_of_a_conversation() {
    let mut mirror = Mirror::default();
    let _ = mirror.apply(&LiveEvent::MirrorComputers {
        computers: vec![computer(vec![session(1)])],
    });
    let _ = mirror.apply(&entries(1, &["made-up request"]));
    let _ = mirror.apply(&LiveEvent::MirrorEntries {
        owner: "laptop-a".to_owned(),
        program: "claude".to_owned(),
        session: session(1).session,
        entries: vec![MirrorEntry::Agent {
            text: "made-up answer".to_owned(),
        }],
        tail: false,
    });
    let view = mirror.snapshot().expect("the mirror changed");

    assert_eq!(
        view.entries(&key(1)),
        [
            Entry::Person {
                text: "made-up request".to_owned(),
                via_front_desk: false,
            },
            Entry::Agent {
                message_id: String::new(),
                text: "made-up answer".to_owned(),
            },
        ],
        "the tail of the follow and then the new rows"
    );
    assert_eq!(
        view.session(&key(1)).map(|row| row.title.as_str()),
        Some("made-up session 1")
    );
    assert_eq!(view.computer("laptop-a"), "Laptop A");
    assert_eq!(
        view.computer("unknown"),
        "unknown",
        "a computer without a list names itself"
    );
}

#[test]
fn a_whole_tail_replaces_the_rows_that_the_window_kept() {
    let mut mirror = Mirror::default();
    let _ = mirror.apply(&entries(1, &["one", "two"]));
    let _ = mirror.apply(&entries(1, &["three"]));
    let view = mirror.snapshot().expect("the mirror changed");
    assert_eq!(
        view.entries(&key(1)),
        [Entry::Person {
            text: "three".to_owned(),
            via_front_desk: false,
        }],
        "the whole tail of a new follow replaces the rows before it"
    );
}

#[test]
fn the_end_of_a_follow_keeps_no_row_of_that_session() {
    let mut mirror = Mirror::default();
    let _ = mirror.apply(&entries(1, &["one"]));
    let _ = mirror.apply(&LiveEvent::MirrorEnded {
        owner: "laptop-a".to_owned(),
        program: "claude".to_owned(),
        session: session(1).session,
        reason: horizon_sync::LiveEndReason::Offline,
    });
    let view = mirror.snapshot().expect("the mirror changed");
    assert!(view.entries(&key(1)).is_empty());
}

#[test]
fn a_frame_of_a_program_that_this_build_does_not_know_is_left_out() {
    let mut mirror = Mirror::default();
    let _ = mirror.apply(&LiveEvent::MirrorEntries {
        owner: "laptop-a".to_owned(),
        program: "another".to_owned(),
        session: session(1).session,
        entries: vec![MirrorEntry::Person {
            text: "made-up request".to_owned(),
        }],
        tail: true,
    });
    assert!(mirror.snapshot().is_none(), "an unknown program changes nothing");
}

#[test]
fn the_rows_of_a_session_are_bounded_and_a_link_that_ends_takes_the_list() {
    let mut mirror = Mirror::default();
    let many: Vec<&str> = vec!["made-up row"; 300];
    let _ = mirror.apply(&entries(1, &many));
    let view = mirror.snapshot().expect("the mirror changed");
    assert_eq!(view.entries(&key(1)).len(), super::MAX_KEPT_ENTRIES);

    let _ = mirror.apply(&LiveEvent::MirrorComputers {
        computers: vec![computer(vec![session(1)])],
    });
    mirror.clear();
    let view = mirror.snapshot().expect("a link that ended changed the list");
    assert!(view.is_empty());
    assert!(view.entries(&key(1)).is_empty());
}

#[test]
fn a_choice_of_the_group_holds_the_computers_that_are_there() {
    let mut choice = SessionMirror::default();
    choice.expand("laptop-a");
    assert!(choice.is_expanded("laptop-a"));
    choice.retain(&[computer(vec![session(1)])]);
    assert!(choice.is_expanded("laptop-a"));
    choice.retain(&[]);
    assert!(!choice.is_expanded("laptop-a"), "a computer that left is forgotten");
}
