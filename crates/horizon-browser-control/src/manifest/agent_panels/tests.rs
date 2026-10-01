use super::*;
use crate::manifest::request_queue::MAX_PENDING_REQUESTS;

fn identity() -> AgentIdentity<'static> {
    AgentIdentity::new("horizon:agent", Some("host-a"))
}

fn send(text: &str) -> Operation {
    Operation::Send {
        panel_id: "panel-1".into(),
        text: text.into(),
        submit: true,
    }
}

#[test]
fn operations_parse_with_defaults_and_refuse_unknown_fields() {
    let list: Operation = serde_json::from_str(r#"{"operation":"list"}"#).unwrap();
    assert_eq!(list, Operation::List);

    let send: Operation = serde_json::from_str(r#"{"operation":"send","panel_id":"p","text":"hi"}"#).unwrap();
    assert_eq!(
        send,
        Operation::Send {
            panel_id: "p".into(),
            text: "hi".into(),
            submit: true
        }
    );

    let read: Operation = serde_json::from_str(r#"{"operation":"read","panel_id":"p"}"#).unwrap();
    assert_eq!(
        read,
        Operation::Read {
            panel_id: "p".into(),
            lines: None
        }
    );

    assert!(serde_json::from_str::<Operation>(r#"{"operation":"send","panel_id":"p","text":"x","extra":1}"#).is_err());
    assert!(serde_json::from_str::<Operation>(r#"{"operation":"delete","panel_id":"p"}"#).is_err());
}

#[test]
fn malformed_operations_are_refused_before_they_are_queued() {
    let root = tempfile::tempdir().unwrap();
    let refused = [
        send("   "),
        send(&"x".repeat(MAX_TEXT_BYTES + 1)),
        Operation::Send {
            panel_id: String::new(),
            text: "hi".into(),
            submit: true,
        },
        Operation::Read {
            panel_id: "p".into(),
            lines: Some(0),
        },
        Operation::Read {
            panel_id: "p".into(),
            lines: Some(MAX_READ_LINES + 1),
        },
        Operation::Read {
            panel_id: "bad\nid".into(),
            lines: None,
        },
    ];
    for operation in refused {
        let error = enqueue_at(root.path(), identity(), operation, Duration::from_secs(5), None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(claim_at(root.path(), "host-a").unwrap().is_empty());
}

#[test]
fn notes_need_a_short_title_and_a_bounded_body() {
    let root = tempfile::tempdir().unwrap();
    let note = |title: &str, markdown: &str| Operation::Note {
        title: title.into(),
        markdown: markdown.into(),
    };
    for refused in [
        note("", "body"),
        note("two\nlines", "body"),
        note(&"t".repeat(MAX_NOTE_TITLE_BYTES + 1), "body"),
        note("title", "  "),
        note("title", &"m".repeat(MAX_NOTE_BYTES + 1)),
    ] {
        let error = enqueue_at(root.path(), identity(), refused, Duration::from_secs(5), None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(
        enqueue_at(
            root.path(),
            identity(),
            note("Summary", "- **done**"),
            Duration::from_secs(5),
            None
        )
        .is_ok()
    );
}

#[test]
fn queue_is_host_bound_single_claim_and_result_is_identity_bound() {
    let root = tempfile::tempdir().unwrap();
    let request = enqueue_at(root.path(), identity(), Operation::List, Duration::from_secs(5), None).unwrap();

    assert!(claim_at(root.path(), "host-b").unwrap().is_empty());
    assert_eq!(claim_at(root.path(), "host-a").unwrap().len(), 1);
    assert!(claim_at(root.path(), "host-a").unwrap().is_empty());

    complete_at(root.path(), &request, Outcome::Panels { panels: vec![] }).unwrap();
    let mut foreign = request.clone();
    foreign.actor = "horizon:other".into();
    assert!(take_result_at(root.path(), &foreign).is_err());
    assert!(take_result_at(root.path(), &request).unwrap().is_some());
    assert!(take_result_at(root.path(), &request).unwrap().is_none());
}

#[test]
fn unbound_callers_and_queue_overflow_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    for caller in [
        AgentIdentity::new("outside", Some("host-a")),
        AgentIdentity::new("horizon:agent", None),
    ] {
        let error = enqueue_at(root.path(), caller, Operation::List, Duration::ZERO, None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
    for _ in 0..MAX_PENDING_REQUESTS {
        enqueue_at(root.path(), identity(), Operation::List, Duration::ZERO, None).unwrap();
    }
    let error = enqueue_at(root.path(), identity(), Operation::List, Duration::ZERO, None).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn outcomes_round_trip_through_the_result_file() {
    let root = tempfile::tempdir().unwrap();
    let request = enqueue_at(root.path(), identity(), Operation::List, Duration::from_secs(5), None).unwrap();
    let panel = AgentPanel {
        panel_id: "p".into(),
        title: "codex".into(),
        kind: "codex".into(),
        state: AgentState::NeedsInput,
        workspace: "api".into(),
        directory: Some("/work".into()),
        is_caller: false,
    };
    complete_at(
        root.path(),
        &request,
        Outcome::Panels {
            panels: vec![panel.clone()],
        },
    )
    .unwrap();
    let Some(Outcome::Panels { panels }) = take_result_at(root.path(), &request).unwrap() else {
        panic!("expected panels");
    };
    assert_eq!(panels, vec![panel]);
}

#[test]
fn printable_keeps_text_newlines_and_tabs_only() {
    assert_eq!(printable("run /compact\nthen\tstop"), "run /compact\nthen\tstop");
    assert_eq!(printable("a\r\nb"), "a\nb");
    assert_eq!(printable("rm\x1b[2Jx\x03y\x07z\r"), "rm[2Jxyz");
}

#[test]
fn a_message_of_only_control_characters_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let error = enqueue_at(
        root.path(),
        identity(),
        send("\x1b\x03\r"),
        Duration::from_secs(5),
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn the_credential_travels_with_the_request_and_is_optional() {
    let root = tempfile::tempdir().unwrap();
    enqueue_at(
        root.path(),
        identity(),
        Operation::Approvals,
        Duration::from_secs(5),
        Some("secret".to_string()),
    )
    .unwrap();
    enqueue_at(root.path(), identity(), Operation::List, Duration::from_secs(5), None).unwrap();
    let mut claimed = claim_at(root.path(), "host-a").unwrap();
    claimed.sort_by_key(|request| request.credential.is_none());
    assert_eq!(claimed[0].credential.as_deref(), Some("secret"));
    assert_eq!(claimed[1].credential, None);
}

fn step(title: &str, detail: Option<&str>) -> PlanStep {
    PlanStep {
        title: title.to_string(),
        detail: detail.map(str::to_string),
        status: StepStatus::Pending,
    }
}

#[test]
fn a_plan_parses_and_an_empty_plan_clears() {
    let operation: Operation = serde_json::from_value(serde_json::json!({
        "operation": "plan",
        "steps": [
            {"title": "Set cloud auto-stop", "detail": "120 -> 90 min", "status": "done"},
            {"title": "Report back", "status": "pending"}
        ]
    }))
    .expect("plan");
    assert!(operation.validate().is_ok());
    let clear = Operation::Plan { steps: Vec::new() };
    assert!(clear.validate().is_ok());
    assert!(
        serde_json::from_value::<Operation>(
            serde_json::json!({"operation": "plan", "steps": [{"title": "x", "status": "soon"}]})
        )
        .is_err()
    );
}

#[test]
fn plan_steps_are_short_single_lines_and_bounded() {
    let refused = |steps: Vec<PlanStep>| Operation::Plan { steps }.validate().is_err();
    assert!(refused(vec![step("  ", None)]));
    assert!(refused(vec![step("two\nlines", None)]));
    assert!(refused(vec![step(&"t".repeat(MAX_STEP_TITLE_BYTES + 1), None)]));
    assert!(refused(vec![step("ok", Some(&"d".repeat(MAX_STEP_DETAIL_BYTES + 1)))]));
    assert!(refused(vec![step("ok", Some("a\nb"))]));
    assert!(refused(
        (0..=MAX_PLAN_STEPS).map(|i| step(&format!("step {i}"), None)).collect()
    ));
    assert!(!refused(
        (0..MAX_PLAN_STEPS).map(|i| step(&format!("step {i}"), None)).collect()
    ));
}
