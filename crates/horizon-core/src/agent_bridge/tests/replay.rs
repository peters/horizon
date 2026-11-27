//! The replay of the captured lines of the program: every line of the folder
//! that `HORIZON_BRIDGE_CAPTURES` names goes through one codec and one
//! conversation, and the test asserts what the real runs proved about the line
//! format.
//!
//! The captures are the standard output of real runs, so they hold the data of
//! the machine that made them and no line of them is part of this repository.
//! This test therefore prints counts and the names of the kinds only, and never
//! the text of a line. It is `#[ignore]`: without the variable it returns at
//! once, and no test of this module starts a program.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::agent_bridge::{
    BridgeEvent, ClaudeCodec, Conversation, ConversationSnapshot, DropReason, Entry, LineCodec, MAX_LINE_BYTES,
    ToolState,
};

/// The variable that names the folder of the captured lines.
const CAPTURES: &str = "HORIZON_BRIDGE_CAPTURES";

/// The longest name of a `type` or of a `subtype` that this test prints, so a
/// line that invents one cannot print a page.
const MAX_NAME_CHARS: usize = 40;

#[test]
#[ignore = "needs HORIZON_BRIDGE_CAPTURES, a folder of captured lines"]
fn the_captured_lines_meet_the_expectations_of_the_program() {
    let Some(folder) = std::env::var_os(CAPTURES) else {
        return;
    };
    let files =
        capture_files(Path::new(&folder)).unwrap_or_else(|error| panic!("cannot read the capture folder: {error}"));
    assert!(!files.is_empty(), "the folder holds no .jsonl capture");
    let reports = replay_all(&files);
    print_pairs(&pair_table(&reports));
    check_common(&reports);
    check_two_turns(&reports);
    check_partial_messages(&reports);
    check_tool_call(&reports);
    check_resume(&reports);
    check_killed_turn(&reports);
    check_denials(&reports);
}

/// One `(type, subtype)` pair of a line, and what the codec did with it.
#[derive(Default)]
struct Pair {
    lines: usize,
    events: BTreeMap<&'static str, usize>,
    files: Vec<String>,
}

/// What one capture gave.
#[derive(Default)]
struct Report {
    lines: usize,
    events: BTreeMap<&'static str, usize>,
    dropped: BTreeMap<&'static str, usize>,
    pairs: BTreeMap<(String, String), Pair>,
    tool_starts: Vec<ToolStart>,
    turns_ended: usize,
    turns_failed: usize,
    cost_usd: Option<f64>,
    snapshot: Option<Arc<ConversationSnapshot>>,
}

/// One tool call that the program started.
struct ToolStart {
    name: String,
    summary: String,
}

impl Report {
    /// The conversation of this capture.
    fn snapshot(&self) -> &ConversationSnapshot {
        match &self.snapshot {
            Some(snapshot) => snapshot,
            None => panic!("the report holds no conversation"),
        }
    }

    /// The entries of the conversation.
    fn entries(&self) -> &[Entry] {
        &self.snapshot().entries
    }

    /// How many times the codec gave one kind of event.
    fn event(&self, kind: &str) -> usize {
        self.events.get(kind).copied().unwrap_or(0)
    }

    /// The texts of the messages of the agent, in the order of the conversation.
    fn agent_texts(&self) -> Vec<&str> {
        self.entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Agent { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Records the events of one line and applies them to the conversation.
    fn record(&mut self, (kind, subtype): (String, String), events: Vec<BridgeEvent>, conversation: &mut Conversation) {
        let pair = self.pairs.entry((kind, subtype)).or_default();
        pair.lines += 1;
        for event in &events {
            *pair.events.entry(kind_of(event)).or_default() += 1;
            *self.events.entry(kind_of(event)).or_default() += 1;
            match event {
                BridgeEvent::Dropped { reason } => *self.dropped.entry(reason_name(*reason)).or_default() += 1,
                BridgeEvent::ToolStarted { name, summary, .. } => self.tool_starts.push(ToolStart {
                    name: name.clone(),
                    summary: summary.clone(),
                }),
                BridgeEvent::TurnEnded { failed, cost_usd, .. } => {
                    self.turns_ended += 1;
                    self.turns_failed += usize::from(*failed);
                    if let Some(cost) = cost_usd {
                        *self.cost_usd.get_or_insert(0.0) += *cost;
                    }
                }
                _ => {}
            }
        }
        for event in events {
            conversation.apply(event);
        }
    }
}

/// The captured files of one folder, in the order of their names.
fn capture_files(folder: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// The report of each capture, in the order of the folder.
fn replay_all(files: &[PathBuf]) -> BTreeMap<String, Report> {
    let mut reports = BTreeMap::new();
    for path in files {
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default().to_owned();
        let report = replay_file(path).unwrap_or_else(|error| panic!("cannot read {name}: {error}"));
        print_report(&name, &report);
        reports.insert(name, report);
    }
    reports
}

/// The report of one capture: every line, in order, through one codec and one
/// conversation.
fn replay_file(path: &Path) -> io::Result<Report> {
    let bytes = std::fs::read(path)?;
    let mut report = Report::default();
    let mut codec = ClaudeCodec::new();
    let mut conversation = Conversation::new();
    for raw in bytes.split_inclusive(|byte| *byte == b'\n') {
        let line = raw.strip_suffix(b"\n".as_slice()).unwrap_or(raw);
        report.lines += 1;
        let (pair, events) = line_events(&mut codec, line);
        report.record(pair, events, &mut conversation);
    }
    report.snapshot = Some(conversation.snapshot());
    Ok(report)
}

/// The events of one line, as the reader of a bridge gives them: a line above
/// the bound is dropped whole, a line that is not UTF-8 is not JSON, and every
/// other line goes to the codec.
fn line_events(codec: &mut ClaudeCodec, raw: &[u8]) -> ((String, String), Vec<BridgeEvent>) {
    if raw.len() > MAX_LINE_BYTES {
        let pair = ("<too long>".to_owned(), "-".to_owned());
        return (
            pair,
            vec![BridgeEvent::Dropped {
                reason: DropReason::TooLong,
            }],
        );
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        let pair = ("<not utf8>".to_owned(), "-".to_owned());
        return (
            pair,
            vec![BridgeEvent::Dropped {
                reason: DropReason::NotJson,
            }],
        );
    };
    (pair_of(text), codec.parse(text))
}

/// The `(type, subtype)` pair of one line: its two discriminator names, and
/// nothing else of the line.
fn pair_of(line: &str) -> (String, String) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return ("<not json>".to_owned(), "-".to_owned());
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("<no type>");
    let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("-");
    (cut_name(kind), cut_name(subtype))
}

/// One discriminator name, which is a name of the program and not its data.
fn cut_name(name: &str) -> String {
    name.chars().take(MAX_NAME_CHARS).collect()
}

/// The name of one kind of event, as this test prints it.
fn kind_of(event: &BridgeEvent) -> &'static str {
    match event {
        BridgeEvent::SessionStarted { .. } => "SessionStarted",
        BridgeEvent::TurnStarted => "TurnStarted",
        BridgeEvent::AgentText { .. } => "AgentText",
        BridgeEvent::ToolStarted { .. } => "ToolStarted",
        BridgeEvent::ToolEnded { .. } => "ToolEnded",
        BridgeEvent::PermissionDenied { .. } => "PermissionDenied",
        BridgeEvent::Limit { .. } => "Limit",
        BridgeEvent::TurnEnded { .. } => "TurnEnded",
        BridgeEvent::Dropped { .. } => "Dropped",
        BridgeEvent::Ended { .. } => "Ended",
    }
}

/// The name of one reason of a dropped line.
fn reason_name(reason: DropReason) -> &'static str {
    match reason {
        DropReason::TooLong => "TooLong",
        DropReason::NotJson => "NotJson",
        DropReason::UnknownShape => "UnknownShape",
    }
}

/// The counts of one map, as text, or `none` when it is empty.
fn names<K: std::fmt::Display>(counts: &BTreeMap<K, usize>) -> String {
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(kind, count)| format!("{kind} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The entries of one report, by kind.
fn entry_counts(report: &Report) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for entry in report.entries() {
        let kind = match entry {
            Entry::Person { .. } => "Person",
            Entry::Agent { .. } => "Agent",
            Entry::Tool { .. } => "Tool",
            Entry::Notice { .. } => "Notice",
        };
        *counts.entry(kind).or_default() += 1;
    }
    counts
}

/// The pairs of one capture that gave no event.
fn silent_pairs(report: &Report) -> String {
    let counts: BTreeMap<String, usize> = report
        .pairs
        .iter()
        .filter(|(_, pair)| pair.events.is_empty())
        .map(|((kind, subtype), pair)| (format!("{kind}/{subtype}"), pair.lines))
        .collect();
    names(&counts)
}

/// One block for one capture.
fn print_report(name: &str, report: &Report) {
    println!("{name}: {} lines", report.lines);
    println!("  events: {}", names(&report.events));
    println!("  dropped by reason: {}", names(&report.dropped));
    println!("  no event: {}", silent_pairs(report));
    println!("  entries: {}", names(&entry_counts(report)));
    println!("  turns ended: {} (failed {})", report.turns_ended, report.turns_failed);
    println!(
        "  session id: {}",
        if report.snapshot().session_id.is_some() {
            "found"
        } else {
            "none"
        }
    );
    println!("  busy at the end: {}", report.snapshot().busy);
    match report.cost_usd {
        Some(cost) => println!("  cost of the ended turns: {cost:.6} (the conversation holds no cost)"),
        None => println!("  cost of the ended turns: none (the conversation holds no cost)"),
    }
}

/// Every pair of every capture, with the captures that hold it.
fn pair_table(reports: &BTreeMap<String, Report>) -> BTreeMap<(String, String), Pair> {
    let mut table: BTreeMap<(String, String), Pair> = BTreeMap::new();
    for (name, report) in reports {
        for (pair, count) in &report.pairs {
            let row = table.entry(pair.clone()).or_default();
            row.lines += count.lines;
            row.files.push(name.clone());
            for (kind, events) in &count.events {
                *row.events.entry(*kind).or_default() += *events;
            }
        }
    }
    table
}

/// Every pair of every capture with what the codec does with it.
fn print_pairs(table: &BTreeMap<(String, String), Pair>) {
    println!("pairs of the captures, and what the codec does with each:");
    for ((kind, subtype), pair) in table {
        let action = if pair.events.is_empty() {
            "no event".to_owned()
        } else {
            names(&pair.events)
        };
        println!(
            "  {kind}/{subtype}: {} lines in {} -> {action}",
            pair.lines,
            pair.files.join(" ")
        );
    }
}

/// The report of one capture.
fn capture<'a>(reports: &'a BTreeMap<String, Report>, name: &str) -> &'a Report {
    match reports.get(name) {
        Some(report) => report,
        None => panic!("the folder holds no {name}"),
    }
}

/// Every capture: no line of the real program is JSON garbage, none is above the
/// bound of one line, and a turn that ended named the account and its limit.
fn check_common(reports: &BTreeMap<String, Report>) {
    for (name, report) in reports {
        assert_eq!(
            report.dropped.get("NotJson"),
            None,
            "{name} gave a line that is not JSON"
        );
        assert_eq!(
            report.dropped.get("TooLong"),
            None,
            "{name} gave a line above the bound"
        );
        if report.turns_ended > 0 {
            assert!(report.event("Limit") > 0, "{name} ended a turn with no limit event");
            assert!(
                report.snapshot().session_id.is_some(),
                "{name} ended a turn without a session id"
            );
        }
    }
}

/// c1: one process served two turns, and the answer of each turn is its own text.
fn check_two_turns(reports: &BTreeMap<String, Report>) {
    let report = capture(reports, "c1.jsonl");
    assert!(report.event("SessionStarted") >= 1, "c1 named no session");
    assert_eq!(report.turns_ended, 2, "c1 did not end two turns");
    assert_eq!(report.turns_failed, 0, "c1 failed a turn");
    assert_eq!(
        report.agent_texts(),
        vec!["alpha", "alpha bravo"],
        "c1 does not give one text per turn"
    );
    assert!(!report.snapshot().busy, "c1 is still busy after both turns");
}

/// c2: the partial messages surround one whole message, which stays one text.
fn check_partial_messages(reports: &BTreeMap<String, Report>) {
    let report = capture(reports, "c2.jsonl");
    assert_eq!(report.agent_texts(), vec!["alpha"], "c2 does not give exactly one text");
    assert_eq!(report.event("AgentText"), 1, "c2 gave more than one text event");
    assert_eq!(report.turns_ended, 1, "c2 did not end one turn");
}

/// c3 and c4: a Bash call runs to its result, and the agent answers after it.
fn check_tool_call(reports: &BTreeMap<String, Report>) {
    for name in ["c3.jsonl", "c4.jsonl"] {
        let report = capture(reports, name);
        let start = report
            .tool_starts
            .iter()
            .find(|start| start.name == "Bash")
            .unwrap_or_else(|| panic!("{name} started no Bash call"));
        assert!(
            start.summary.contains("echo probe-ok"),
            "{name}: the start lost the command"
        );
        let entries = report.entries();
        let tool = entries
            .iter()
            .position(|entry| matches!(entry, Entry::Tool { name: tool, .. } if tool == "Bash"))
            .unwrap_or_else(|| panic!("{name} shows no Bash entry"));
        let agent = entries
            .iter()
            .position(|entry| matches!(entry, Entry::Agent { .. }))
            .unwrap_or_else(|| panic!("{name} shows no agent entry"));
        assert!(agent > tool, "{name}: the answer is not after the call");
        match &entries[tool] {
            Entry::Tool { state, summary, .. } => {
                assert_eq!(*state, ToolState::Done, "{name}: the Bash call did not finish");
                assert!(
                    summary.contains("probe-ok"),
                    "{name}: the result of the Bash call is gone"
                );
            }
            _ => panic!("{name}: the Bash entry is not a tool"),
        }
    }
}

/// c5: a resumed process reports the session of the conversation that it continues.
fn check_resume(reports: &BTreeMap<String, Report>) {
    let report = capture(reports, "c5.jsonl");
    assert_eq!(report.turns_ended, 1, "c5 did not end one turn");
    assert!(report.snapshot().session_id.is_some(), "c5 named no session");
    assert!(report.event("SessionStarted") >= 1, "c5 gave no session start");
}

/// c6b: a killed turn writes no result line, so nothing ends its turn.
fn check_killed_turn(reports: &BTreeMap<String, Report>) {
    let report = capture(reports, "c6b.jsonl");
    assert_eq!(report.turns_ended, 0, "c6b ended a turn although it was killed");
    assert!(
        report.snapshot().busy,
        "c6b is not busy although no line ended the turn"
    );
    assert!(report.event("SessionStarted") >= 1, "c6b named no session");
}

/// c7b: a refused tool call fails that call, and the turn still ends.
fn check_denials(reports: &BTreeMap<String, Report>) {
    let report = capture(reports, "c7b.jsonl");
    assert!(report.event("PermissionDenied") >= 1, "c7b gave no refusal");
    let failed = report
        .entries()
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                Entry::Tool {
                    state: ToolState::Failed,
                    ..
                }
            )
        })
        .count();
    assert!(failed >= 1, "c7b shows no failed tool call");
    assert_eq!(report.turns_ended, 1, "c7b did not end its turn");
}
