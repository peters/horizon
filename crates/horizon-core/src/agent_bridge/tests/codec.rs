//! The codec: each mapping of the line format of Claude Code, the join of the
//! texts of one message, the summary of a tool, the lines that give nothing and
//! the lines that it refuses.

use serde_json::Value;

use crate::agent_bridge::{BridgeEvent, ClaudeCodec, DropReason, LineCodec, MAX_SUMMARY_CHARS};

const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The events of one line.
fn parse(line: &str) -> Vec<BridgeEvent> {
    ClaudeCodec::new().parse(line)
}

#[test]
fn an_init_line_starts_the_session_and_the_turn() {
    let line = format!(r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"a-model"}}"#);
    assert_eq!(
        parse(&line),
        vec![
            BridgeEvent::SessionStarted {
                session_id: SESSION.to_owned()
            },
            BridgeEvent::TurnStarted,
        ]
    );
}

#[test]
fn an_assistant_text_line_gives_the_message_and_its_text() {
    let line = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"alpha"}]},"session_id":"a-session"}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha".to_owned(),
        }]
    );
}

#[test]
fn the_texts_of_one_message_join_by_its_id() {
    let mut codec = ClaudeCodec::new();
    let first = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"alpha"}]}}"#;
    let second = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":" bravo"}]}}"#;
    let other = r#"{"type":"assistant","message":{"id":"msg-2","content":[{"type":"text","text":"charlie"}]}}"#;
    assert_eq!(
        codec.parse(first),
        vec![BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha".to_owned(),
        }]
    );
    assert_eq!(
        codec.parse(second),
        vec![BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha bravo".to_owned(),
        }]
    );
    assert_eq!(
        codec.parse(other),
        vec![BridgeEvent::AgentText {
            message_id: "msg-2".to_owned(),
            text: "charlie".to_owned(),
        }]
    );
}

#[test]
fn a_tool_call_gives_one_start_with_a_fixed_key_of_its_input() {
    let line = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo alpha","description":"a private note"}}]}}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::ToolStarted {
            tool_id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            summary: "echo alpha".to_owned(),
        }]
    );
}

#[test]
fn a_file_tool_summarises_its_path_and_another_tool_its_name() {
    let file = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/tmp/alpha.txt","offset":10}}]}}"#;
    assert_eq!(
        parse(file),
        vec![BridgeEvent::ToolStarted {
            tool_id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            summary: "/tmp/alpha.txt".to_owned(),
        }]
    );
    let other = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"tool_use","id":"tool-2","name":"Task","input":{"prompt":"a secret note"}}]}}"#;
    assert_eq!(
        parse(other),
        vec![BridgeEvent::ToolStarted {
            tool_id: "tool-2".to_owned(),
            name: "Task".to_owned(),
            summary: "Task".to_owned(),
        }]
    );
}

#[test]
fn a_tool_summary_is_cleaned_and_cut() {
    let command = format!("echo\talpha\n{}", "a".repeat(500));
    let input = serde_json::json!({ "command": command, "description": "a private note" });
    let line = format!(
        r#"{{"type":"assistant","message":{{"id":"msg-1","content":[{{"type":"tool_use","id":"tool-1","name":"Bash","input":{input}}}]}}}}"#
    );
    let events = parse(&line);
    let [BridgeEvent::ToolStarted { summary, .. }] = events.as_slice() else {
        panic!("the line gave {events:?}");
    };
    assert!(!summary.contains('\t'), "{summary}");
    assert!(!summary.contains('\n'), "{summary}");
    assert!(!summary.contains("private"), "{summary}");
    assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
    assert!(summary.starts_with("echo alpha "), "{summary}");
}

#[test]
fn a_tool_result_gives_the_end_of_its_tool_with_the_first_line() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"tool-1","type":"tool_result","content":"alpha\nbravo","is_error":true}]}}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::ToolEnded {
            tool_id: "tool-1".to_owned(),
            failed: true,
            summary: "alpha".to_owned(),
        }]
    );
    let refused = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"tool-2","type":"tool_result","content":[{"type":"text","text":"charlie"}],"is_error":false}]}}"#;
    assert_eq!(
        parse(refused),
        vec![BridgeEvent::ToolEnded {
            tool_id: "tool-2".to_owned(),
            failed: false,
            summary: "charlie".to_owned(),
        }]
    );
}

#[test]
fn a_permission_denial_gives_its_tool_and_its_reason() {
    let line = r#"{"type":"system","subtype":"permission_denied","tool_name":"Write","tool_use_id":"tool-1","message":"the folder needs approval"}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::PermissionDenied {
            tool: "Write".to_owned(),
            message: "the folder needs approval".to_owned(),
        }]
    );
}

#[test]
fn a_rate_limit_line_gives_the_window_of_the_account() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1700000000,"isUsingOverage":true}}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::Limit {
            status: "rejected".to_owned(),
            using_overage: true,
            resets_at_unix: Some(1_700_000_000),
        }]
    );
}

#[test]
fn a_result_line_ends_the_turn_with_its_own_cost() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1479,"total_cost_usd":1.75,"model_usage_since_last_result":{"first-model":{"costUSD":0.25},"second-model":{"costUSD":0.5}}}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: Some(1479),
            cost_usd: Some(0.75),
        }]
    );
}

#[test]
fn a_failed_result_line_says_so() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;
    assert_eq!(
        parse(line),
        vec![BridgeEvent::TurnEnded {
            failed: true,
            duration_ms: None,
            cost_usd: None,
        }]
    );
}

#[test]
fn a_type_that_this_slice_does_not_use_gives_nothing() {
    for line in [
        r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup"}"#,
        r#"{"type":"system","subtype":"hook_response","output":"alpha"}"#,
        r#"{"type":"system","subtype":"status","status":"requesting"}"#,
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":50}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"a_type_of_a_later_version","text":"alpha"}"#,
    ] {
        assert!(parse(line).is_empty(), "{line}");
    }
}

#[test]
fn a_line_that_is_not_json_is_dropped_whole() {
    assert_eq!(
        parse("alpha bravo"),
        vec![BridgeEvent::Dropped {
            reason: DropReason::NotJson
        }]
    );
    assert_eq!(
        parse(r#"{"type":"assistant","#),
        vec![BridgeEvent::Dropped {
            reason: DropReason::NotJson
        }]
    );
}

#[test]
fn a_json_line_without_a_type_is_a_shape_that_the_codec_does_not_know() {
    for line in [r#"{"message":"alpha"}"#, r#"["alpha"]"#, r#"{"type":7}"#] {
        assert_eq!(
            parse(line),
            vec![BridgeEvent::Dropped {
                reason: DropReason::UnknownShape
            }],
            "{line}"
        );
    }
}

#[test]
fn a_thinking_block_of_a_message_gives_no_text_and_keeps_the_text_of_its_message() {
    let mut codec = ClaudeCodec::new();
    let thinking = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"thinking","thinking":"a private note","signature":"a-signature"}]}}"#;
    assert!(codec.parse(thinking).is_empty(), "{thinking}");
    let text = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"alpha"}]}}"#;
    assert_eq!(
        codec.parse(text),
        vec![BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha".to_owned(),
        }]
    );
}

#[test]
fn a_text_delta_gives_no_event_so_the_whole_message_stays_one_text() {
    let mut codec = ClaudeCodec::new();
    let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"al"}},"session_id":"a-session"}"#;
    assert!(codec.parse(delta).is_empty(), "{delta}");
    let whole = r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"alpha"}]}}"#;
    assert_eq!(
        codec.parse(whole),
        vec![BridgeEvent::AgentText {
            message_id: "msg-1".to_owned(),
            text: "alpha".to_owned(),
        }]
    );
}

#[test]
fn the_user_line_is_one_line_of_the_shape_of_the_program() {
    let text = "alpha \"bravo\"\ncharlie — δοκιμή\t";
    let line = ClaudeCodec::new().user_line(text);
    assert!(!line.contains('\n'), "{line}");
    assert!(line.contains("\\n"), "{line}");
    assert!(line.contains("\\\"bravo\\\""), "{line}");
    let parsed: Value = serde_json::from_str(&line).expect("the user line is JSON");
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"][0]["type"], "text");
    assert_eq!(parsed["message"]["content"][0]["text"], text);
}

#[test]
fn an_id_of_the_program_is_cut_when_it_enters_an_event() {
    let long = "a".repeat(5000);
    let cut = "a".repeat(128);
    let line = format!(
        r#"{{"type":"assistant","message":{{"id":"{long}","content":[{{"type":"text","text":"alpha"}},{{"type":"tool_use","id":"{long}","name":"Bash","input":{{"command":"echo alpha"}}}}]}}}}"#
    );
    let events = parse(&line);
    let [
        BridgeEvent::AgentText { message_id, .. },
        BridgeEvent::ToolStarted { tool_id, .. },
    ] = events.as_slice()
    else {
        panic!("the line gave {events:?}");
    };
    assert_eq!(message_id, &cut);
    assert_eq!(tool_id, &cut);
    let result = format!(
        r#"{{"type":"user","message":{{"content":[{{"tool_use_id":"{long}","type":"tool_result","content":"alpha"}}]}}}}"#
    );
    assert_eq!(
        parse(&result),
        vec![BridgeEvent::ToolEnded {
            tool_id: cut,
            failed: false,
            summary: "alpha".to_owned(),
        }],
        "the end of the tool joins the row that its start made"
    );
}

#[test]
fn a_session_id_that_is_not_of_the_form_of_a_uuid_is_a_shape_the_codec_does_not_know() {
    for id in [
        "--dangerously-skip-permissions",
        "--resume",
        "",
        "a-session",
        "00000000-0000-4000-8000-00000000000",
        "00000000-0000-4000-8000-0000000000011",
        "000000000000-4000-8000-000000000001",
        "00000000_0000-4000-8000-000000000001",
    ] {
        let line = format!(r#"{{"type":"system","subtype":"init","session_id":"{id}"}}"#);
        assert_eq!(
            parse(&line),
            vec![BridgeEvent::Dropped {
                reason: DropReason::UnknownShape
            }],
            "{id}"
        );
    }
    let huge = "a".repeat(4096);
    let line = format!(r#"{{"type":"system","subtype":"init","session_id":"{huge}"}}"#);
    assert_eq!(
        parse(&line),
        vec![BridgeEvent::Dropped {
            reason: DropReason::UnknownShape
        }],
        "a value above the length of one argument"
    );
    let upper = r#"{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-00000000000A"}"#;
    assert_eq!(
        parse(upper),
        vec![
            BridgeEvent::SessionStarted {
                session_id: "00000000-0000-4000-8000-00000000000A".to_owned()
            },
            BridgeEvent::TurnStarted,
        ],
        "a hexadecimal digit of either case is of the form"
    );
}
