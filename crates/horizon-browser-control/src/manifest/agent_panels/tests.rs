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
        let error = enqueue_at(root.path(), identity(), operation, Duration::from_secs(5)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(claim_at(root.path(), "host-a").unwrap().is_empty());
}

#[test]
fn queue_is_host_bound_single_claim_and_result_is_identity_bound() {
    let root = tempfile::tempdir().unwrap();
    let request = enqueue_at(root.path(), identity(), Operation::List, Duration::from_secs(5)).unwrap();

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
        let error = enqueue_at(root.path(), caller, Operation::List, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
    for _ in 0..MAX_PENDING_REQUESTS {
        enqueue_at(root.path(), identity(), Operation::List, Duration::ZERO).unwrap();
    }
    let error = enqueue_at(root.path(), identity(), Operation::List, Duration::ZERO).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn outcomes_round_trip_through_the_result_file() {
    let root = tempfile::tempdir().unwrap();
    let request = enqueue_at(root.path(), identity(), Operation::List, Duration::from_secs(5)).unwrap();
    let panel = AgentPanel {
        panel_id: "p".into(),
        title: "codex".into(),
        kind: "codex".into(),
        state: AgentState::NeedsInput,
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
