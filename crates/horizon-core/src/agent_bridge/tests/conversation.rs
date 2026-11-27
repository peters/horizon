//! The conversation: the replacement of a text by its message id, the state of a
//! tool, the cap of the list with its notice, the busy state and the version.

use crate::agent_bridge::{
    BridgeEvent, Conversation, ConversationSnapshot, Entry, LimitNote, Link, MAX_ENTRIES, ToolState,
};

/// The conversation of the events of one test.
fn conversation(events: Vec<BridgeEvent>) -> Conversation {
    let mut conversation = Conversation::new();
    for event in events {
        conversation.apply(event);
    }
    conversation
}

/// The entries of a conversation.
fn entries(conversation: &Conversation) -> Vec<Entry> {
    conversation.snapshot().entries.clone()
}

#[test]
fn a_text_with_a_known_message_id_replaces_the_earlier_text() {
    let conversation = conversation(vec![
        BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha".to_owned(),
        },
        BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha bravo".to_owned(),
        },
    ]);
    assert_eq!(
        entries(&conversation),
        vec![Entry::Agent {
            message_id: "msg-1".to_owned(),
            text: "alpha bravo".to_owned(),
        }]
    );
    let snapshot = conversation.snapshot();
    assert_eq!(snapshot.version, 2);
    assert_eq!(
        ConversationSnapshot {
            version: 2,
            link: Link::NotStarted,
            session_id: None,
            entries: snapshot.entries.clone(),
            busy: false,
            limit: None,
            cost_usd_total: 0.0,
        },
        *snapshot
    );
}

#[test]
fn a_tool_walks_from_running_to_done_or_to_failed() {
    let conversation = conversation(vec![
        BridgeEvent::ToolStarted {
            tool_id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            summary: "echo alpha".to_owned(),
        },
        BridgeEvent::ToolStarted {
            tool_id: "tool-2".to_owned(),
            name: "Read".to_owned(),
            summary: "/tmp/alpha.txt".to_owned(),
        },
        BridgeEvent::ToolEnded {
            tool_id: "tool-1".to_owned(),
            failed: false,
            summary: "alpha".to_owned(),
        },
        BridgeEvent::ToolEnded {
            tool_id: "tool-2".to_owned(),
            failed: true,
            summary: String::new(),
        },
    ]);
    assert_eq!(
        entries(&conversation),
        vec![
            Entry::Tool {
                tool_id: "tool-1".to_owned(),
                name: "Bash".to_owned(),
                summary: "echo alpha".to_owned(),
                result: Some("alpha".to_owned()),
                state: ToolState::Done,
            },
            Entry::Tool {
                tool_id: "tool-2".to_owned(),
                name: "Read".to_owned(),
                summary: "/tmp/alpha.txt".to_owned(),
                result: None,
                state: ToolState::Failed,
            },
        ]
    );
}

#[test]
fn the_result_of_a_tool_is_its_first_line_and_the_summary_keeps_the_command() {
    let conversation = conversation(vec![
        BridgeEvent::ToolStarted {
            tool_id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            summary: "exit 1".to_owned(),
        },
        BridgeEvent::ToolEnded {
            tool_id: "tool-1".to_owned(),
            failed: true,
            summary: "line one\nline two".to_owned(),
        },
    ]);
    assert_eq!(
        entries(&conversation),
        vec![Entry::Tool {
            tool_id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            summary: "exit 1".to_owned(),
            result: Some("line one".to_owned()),
            state: ToolState::Failed,
        }]
    );
}

#[test]
fn the_end_of_a_tool_that_the_conversation_did_not_see_start_changes_nothing() {
    let conversation = conversation(vec![BridgeEvent::ToolEnded {
        tool_id: "tool-1".to_owned(),
        failed: false,
        summary: "alpha".to_owned(),
    }]);
    assert!(entries(&conversation).is_empty());
    assert_eq!(conversation.snapshot().version, 1);
}

#[test]
fn the_list_keeps_its_bound_and_one_notice_of_what_it_dropped() {
    let mut conversation = Conversation::new();
    for number in 0..MAX_ENTRIES + 3 {
        conversation.person_said(&format!("message {number}"));
    }
    let kept = entries(&conversation);
    assert_eq!(kept.len(), MAX_ENTRIES);
    assert_eq!(
        kept.first(),
        Some(&Entry::Notice {
            text: "4 earlier entries were dropped".to_owned()
        })
    );
    assert_eq!(
        kept.last(),
        Some(&Entry::Person {
            text: format!("message {}", MAX_ENTRIES + 2)
        })
    );
    conversation.person_said("one more");
    let kept = entries(&conversation);
    assert_eq!(kept.len(), MAX_ENTRIES);
    assert_eq!(
        kept.first(),
        Some(&Entry::Notice {
            text: "5 earlier entries were dropped".to_owned()
        })
    );
}

#[test]
fn the_cost_of_every_ended_turn_is_summed_and_a_turn_without_a_cost_adds_nothing() {
    let mut conversation = Conversation::new();
    for cost_usd in [Some(0.004_168), Some(0.006_879), None, Some(0.01)] {
        conversation.apply(BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: Some(5),
            cost_usd,
        });
    }
    assert!(
        (conversation.snapshot().cost_usd_total - 0.021_047).abs() < 1e-9,
        "the cost total is {}",
        conversation.snapshot().cost_usd_total
    );
}

#[test]
fn a_notice_of_the_window_is_an_entry_and_keeps_the_busy_state() {
    let mut conversation = Conversation::new();
    conversation.person_said("alpha");
    let version = conversation.snapshot().version;
    conversation.notice("Stopped by you");
    assert!(conversation.snapshot().busy);
    assert_eq!(conversation.snapshot().version, version + 1);
    assert_eq!(
        conversation.snapshot().entries.last(),
        Some(&Entry::Notice {
            text: "Stopped by you".to_owned()
        })
    );
}

#[test]
fn a_turn_makes_the_conversation_busy_and_its_end_clears_that() {
    let mut conversation = Conversation::new();
    conversation.person_said("alpha");
    assert!(conversation.snapshot().busy);
    conversation.apply(BridgeEvent::TurnEnded {
        failed: false,
        duration_ms: Some(5),
        cost_usd: None,
    });
    assert!(!conversation.snapshot().busy);
    assert_eq!(
        conversation.snapshot().entries.last(),
        Some(&Entry::Person {
            text: "alpha".to_owned()
        })
    );
    conversation.apply(BridgeEvent::TurnStarted);
    assert!(conversation.snapshot().busy);
    conversation.apply(BridgeEvent::Ended {
        exit_code: Some(0),
        stderr_tail: String::new(),
    });
    let snapshot = conversation.snapshot();
    assert!(!snapshot.busy);
    assert_eq!(snapshot.link, Link::Ended { exit_code: Some(0) });
    assert_eq!(
        snapshot.entries.last(),
        Some(&Entry::Notice {
            text: "the program ended with code 0".to_owned()
        })
    );
}

#[test]
fn the_session_the_limit_and_the_refusal_are_fields_of_the_snapshot() {
    let conversation = conversation(vec![
        BridgeEvent::SessionStarted {
            session_id: "00000000-0000-4000-8000-000000000001".to_owned(),
        },
        BridgeEvent::Limit {
            status: "rejected".to_owned(),
            using_overage: true,
            resets_at_unix: Some(1_700_000_000),
        },
        BridgeEvent::PermissionDenied {
            tool: "Write".to_owned(),
            message: "the folder needs\napproval".to_owned(),
        },
    ]);
    let snapshot = conversation.snapshot();
    assert_eq!(
        snapshot.session_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(snapshot.link, Link::Running);
    assert_eq!(
        snapshot.limit,
        Some(LimitNote {
            status: "rejected".to_owned(),
            using_overage: true,
            resets_at_unix: Some(1_700_000_000),
        })
    );
    assert_eq!(
        snapshot.entries.last(),
        Some(&Entry::Notice {
            text: "Write was denied: the folder needs approval".to_owned()
        })
    );
}

#[test]
fn a_dropped_line_gives_one_notice_that_counts_the_lines() {
    let conversation = conversation(vec![
        BridgeEvent::Dropped {
            reason: crate::agent_bridge::DropReason::NotJson,
        },
        BridgeEvent::Dropped {
            reason: crate::agent_bridge::DropReason::TooLong,
        },
    ]);
    assert_eq!(
        entries(&conversation),
        vec![Entry::Notice {
            text: "2 lines of the program were dropped".to_owned()
        }]
    );
}

#[test]
fn the_version_grows_by_one_for_each_change() {
    let mut conversation = Conversation::new();
    assert_eq!(conversation.snapshot().version, 0);
    conversation.person_said("alpha");
    assert_eq!(conversation.snapshot().version, 1);
    conversation.apply(BridgeEvent::TurnStarted);
    assert_eq!(conversation.snapshot().version, 2);
    let kept = conversation.snapshot();
    conversation.apply(BridgeEvent::TurnEnded {
        failed: true,
        duration_ms: None,
        cost_usd: None,
    });
    assert_eq!(kept.version, 2, "a snapshot of an earlier moment kept its version");
    assert_eq!(conversation.snapshot().version, 3);
    assert_eq!(
        conversation.snapshot().entries.last(),
        Some(&Entry::Notice {
            text: "the turn failed".to_owned()
        })
    );
}

#[test]
fn a_restored_conversation_shows_its_entries_and_runs_no_turn() {
    let history = vec![
        Entry::Person {
            text: "alpha".to_owned(),
        },
        Entry::Agent {
            message_id: "msg-1".to_owned(),
            text: "bravo".to_owned(),
        },
    ];

    let conversation = Conversation::restored(history.clone(), Some("session-1".to_owned()));

    let snapshot = conversation.snapshot();
    assert_eq!(snapshot.entries, history);
    assert_eq!(snapshot.session_id, Some("session-1".to_owned()));
    assert_eq!(
        snapshot.link,
        Link::NotStarted,
        "no program of this run named a session"
    );
    assert!(!snapshot.busy, "no turn of the last run is still in flight");
    assert_eq!(snapshot.cost_usd_total, 0.0, "a stored entry holds no cost");
    assert_eq!(snapshot.version, 1, "a restored conversation has one version");
}

#[test]
fn a_stored_history_goes_above_the_messages_that_the_frame_already_showed() {
    let mut conversation = Conversation::new();
    conversation.person_said("new");

    conversation.prepend(vec![Entry::Person { text: "old".to_owned() }]);

    let snapshot = conversation.snapshot();
    assert_eq!(
        snapshot.entries,
        vec![
            Entry::Person { text: "old".to_owned() },
            Entry::Person { text: "new".to_owned() },
        ]
    );
    assert!(snapshot.busy, "the message that waited for its bridge still waits");
}

#[test]
fn a_restored_history_keeps_the_cap_of_the_conversation() {
    let history: Vec<_> = (0..MAX_ENTRIES + 3)
        .map(|index| Entry::Notice {
            text: format!("entry {index}"),
        })
        .collect();

    let conversation = Conversation::restored(history, None);

    let entries = entries(&conversation);
    assert_eq!(entries.len(), MAX_ENTRIES, "the list keeps one cap of entries");
    assert_eq!(
        entries.first(),
        Some(&Entry::Notice {
            text: "4 earlier entries were dropped".to_owned()
        }),
        "one notice names the entries that the cap dropped"
    );
}
