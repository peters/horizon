//! The lifecycle input of an agent program as that program sends it, the rule that
//! a shape this build does not read leaves its field "not given", and the line that
//! the hook prints when it refuses one.
use std::io;
use std::path::Path;

use horizon_core::PanelKind;
use horizon_core::agent_work::{HookEvent, WorkStore};

use super::HookContext;

/// The five events of one turn of Claude Code 2.1.325, with generic paths. That
/// version sends `background_tasks` and `session_crons` as lists, and `effort` as
/// an object.
const CLAUDE_2_1_325: &str = include_str!("claude-2.1.325.json");
/// The folder and the session of those events.
const CWD: &str = "/tmp/horizon-agent-work";
const SESSION: &str = "00000000-0000-0000-0000-000000000001";

fn events() -> Vec<serde_json::Value> {
    serde_json::from_str(CLAUDE_2_1_325).expect("the fixture of 2.1.325")
}

#[test]
fn the_turn_of_claude_2_1_325_is_accepted_and_its_stop_finishes_the_launch() {
    let temp = tempfile::tempdir().expect("fixture");
    let context = HookContext {
        store: WorkStore::new(temp.path()),
        panel: "panel".into(),
        owner: "owner".into(),
        kind: PanelKind::Claude,
    };
    context
        .store
        .register_owner("panel", PanelKind::Claude, "owner", Some(SESSION), Path::new(CWD))
        .expect("the launch of the panel");
    for (index, event) in events().into_iter().enumerate() {
        let now = i64::try_from(index).expect("the order of the event");
        let bytes = serde_json::to_vec(&event).expect("the event");
        context
            .process(bytes.as_slice(), now)
            .expect("this build reads every event of 2.1.325");
    }
    let record = context.store.read("panel").expect("read").expect("record");
    assert_eq!(record.ledger.state, horizon_core::agent_work::TurnState::Finished);
    assert_eq!(
        record.ledger.background_tasks, 0,
        "an empty list of background tasks is no background work"
    );
}

#[test]
fn an_optional_field_of_a_shape_this_build_does_not_read_is_not_given() {
    let event: HookEvent = serde_json::from_value(serde_json::json!({
        "session_id": "session", "hook_event_name": "Stop", "prompt_id": 7,
        "reason": ["a"], "source": 1, "tool_name": {"name": "Bash"},
        "tool_use_id": false, "background_tasks": "two"
    }))
    .expect("the event is accepted");
    assert!(event.prompt_id.is_none() && event.reason.is_none() && event.source.is_none());
    assert!(event.tool_name.is_none() && event.tool_use_id.is_none() && event.background_tasks.is_none());
}

#[test]
fn the_background_tasks_of_a_stop_are_a_count_or_the_tasks_themselves() {
    let count = |value: serde_json::Value| {
        serde_json::from_value::<HookEvent>(serde_json::json!({
            "session_id": "session", "hook_event_name": "Stop", "background_tasks": value
        }))
        .expect("the event is accepted")
        .background_tasks
    };
    assert_eq!(count(serde_json::json!([])), Some(0));
    assert_eq!(count(serde_json::json!([{"id": "a"}, {"id": "b"}])), Some(2));
    assert_eq!(count(serde_json::json!(3)), Some(3));
    assert_eq!(count(serde_json::json!({"id": "a"})), None);
}

#[test]
fn only_the_identity_of_the_evidence_refuses_an_event() {
    let missing_cwd = serde_json::json!({"session_id": "session", "hook_event_name": "Stop"});
    let temp = tempfile::tempdir().expect("fixture");
    let context = HookContext {
        store: WorkStore::new(temp.path()),
        panel: "panel".into(),
        owner: "owner".into(),
        kind: PanelKind::Claude,
    };
    context
        .store
        .register_owner("panel", PanelKind::Claude, "owner", Some("session"), Path::new(CWD))
        .expect("the launch of the panel");
    let error = context
        .process(serde_json::to_vec(&missing_cwd).expect("input").as_slice(), 1)
        .expect_err("an event without the folder of its evidence is refused");
    assert_eq!(error.to_string(), "invalid lifecycle input");
}

#[test]
fn the_hook_prints_the_message_of_an_error_and_no_text_of_the_event() {
    let refused = io::Error::new(io::ErrorKind::InvalidData, "invalid lifecycle input");
    assert_eq!(super::message(&refused), "invalid lifecycle input");
    // A message of the system is one line and bounded, whatever it names.
    let long = io::Error::other(format!("a line\nand {}", "x".repeat(1000)));
    let line = super::message(&long);
    assert!(!line.contains('\n'), "one line: {line}");
    assert!(line.chars().count() <= 200, "bounded: {}", line.chars().count());
}
