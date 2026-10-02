use std::io::Write;

use horizon_core::PanelKind;

use super::{Event, Tail, parse_line};

fn said(text: &str) -> Event {
    Event::Said(text.to_string())
}

#[test]
fn claude_text_and_tools_become_events() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed it."},{"type":"tool_use","name":"Edit","input":{"file_path":"/repo/src/lib.rs"}},{"type":"tool_use","name":"Bash","input":{"command":"cargo test -p api\nmore","description":"Run the API tests"}}]}}"#;
    assert_eq!(
        parse_line(PanelKind::Claude, line),
        vec![
            said("Fixed it."),
            Event::Did("Edited lib.rs".to_string()),
            Event::Did("Run the API tests".to_string()),
        ]
    );
}

#[test]
fn claude_plan_proposal_becomes_a_plan_event() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"ExitPlanMode","input":{"plan":"Make cleanup safe\n1. Add a lock\n2. Add a stress test"}}]}}"#;
    assert_eq!(
        parse_line(PanelKind::Claude, line),
        vec![Event::Plan(
            "Make cleanup safe\n1. Add a lock\n2. Add a stress test".to_string()
        )]
    );
    let empty = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"ExitPlanMode","input":{"plan":"  "}}]}}"#;
    assert!(parse_line(PanelKind::Claude, empty).is_empty());
}

#[test]
fn claude_person_text_is_kept_but_injected_context_and_tool_results_are_not() {
    let typed = r#"{"type":"user","message":{"content":"fix the tests"}}"#;
    let block = r#"{"type":"user","message":{"content":[{"type":"text","text":"and the docs"}]}}"#;
    let injected = r#"{"type":"user","message":{"content":"<system-reminder>x</system-reminder>"}}"#;
    let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#;
    let meta = r#"{"type":"user","isMeta":true,"message":{"content":"hidden"}}"#;
    assert_eq!(
        parse_line(PanelKind::Claude, typed),
        vec![Event::You("fix the tests".to_string())]
    );
    assert_eq!(
        parse_line(PanelKind::Claude, block),
        vec![Event::You("and the docs".to_string())]
    );
    for line in [injected, result, meta] {
        assert!(parse_line(PanelKind::Claude, line).is_empty(), "{line}");
    }
}

#[test]
fn horizon_actions_and_sidechains_are_left_to_the_host() {
    let horizon = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__plugin_horizon_horizon-browser__agent_panels","input":{"operation":"list"}}]}}"#;
    let side = r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"x"}]}}"#;
    assert!(parse_line(PanelKind::Claude, horizon).is_empty());
    assert!(parse_line(PanelKind::Claude, side).is_empty());
}

#[test]
fn codex_messages_commands_and_patches() {
    let ask = r#"{"type":"event_msg","payload":{"type":"user_message","message":"add a test","images":[]}}"#;
    let reply = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}"#;
    let shell = r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}","call_id":"c"}}"#;
    let patch = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch","call_id":"c","status":"completed"}}"#;
    assert_eq!(
        parse_line(PanelKind::Codex, ask),
        vec![Event::You("add a test".to_string())]
    );
    assert_eq!(parse_line(PanelKind::Codex, reply), vec![said("Done.")]);
    assert_eq!(
        parse_line(PanelKind::Codex, shell),
        vec![Event::Did("Ran bash -lc cargo test".to_string())]
    );
    assert_eq!(
        parse_line(PanelKind::Codex, patch),
        vec![Event::Did("Edited files".to_string())]
    );
}

#[test]
fn garbage_and_other_kinds_give_nothing() {
    assert!(parse_line(PanelKind::Claude, "not json").is_empty());
    assert!(parse_line(PanelKind::Shell, r#"{"type":"user","message":{"content":"x"}}"#).is_empty());
}

#[test]
fn a_tail_follows_only_complete_new_lines() {
    let dir = std::env::temp_dir().join(format!("horizon-events-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("session.jsonl");
    let mut file = std::fs::File::create(&path).expect("create");
    writeln!(file, r#"{{"type":"user","message":{{"content":"one"}}}}"#).expect("write");
    let mut tail = Tail::open(path.clone(), PanelKind::Claude);
    assert_eq!(tail.poll(), vec![Event::You("one".to_string())]);
    assert!(tail.poll().is_empty());
    // A line still being written is not read yet.
    write!(
        file,
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"two"}}"#
    )
    .expect("write");
    assert!(tail.poll().is_empty());
    writeln!(file, "]}}}}").expect("write");
    assert_eq!(tail.poll(), vec![said("two")]);
    std::fs::remove_dir_all(&dir).ok();
}

/// Run by hand against a real transcript: `HORIZON_TRANSCRIPT=<file> HORIZON_KIND=claude|codex cargo test -p horizon-ui --
/// real_transcript --ignored --nocapture`. Prints counts only.
#[test]
#[ignore = "needs a real transcript"]
fn real_transcript_parses() {
    let (Ok(path), Ok(kind)) = (std::env::var("HORIZON_TRANSCRIPT"), std::env::var("HORIZON_KIND")) else {
        return;
    };
    let kind = if kind == "codex" {
        PanelKind::Codex
    } else {
        PanelKind::Claude
    };
    let text = std::fs::read_to_string(path).expect("read transcript");
    let events: Vec<Event> = text.lines().flat_map(|line| parse_line(kind, line)).collect();
    let count = |want: fn(&Event) -> bool| events.iter().filter(|event| want(event)).count();
    println!(
        "lines {} -> you {} said {} did {}",
        text.lines().count(),
        count(|e| matches!(e, Event::You(_))),
        count(|e| matches!(e, Event::Said(_))),
        count(|e| matches!(e, Event::Did(_))),
    );
    assert!(!events.is_empty());
}
