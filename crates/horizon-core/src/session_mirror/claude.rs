//! The entries of one Claude Code session, from the transcript that the session
//! pickup names.
//!
//! A transcript is one JSON object on each line. The shapes that this build reads,
//! measured on the transcripts of this computer, are:
//!
//! - `{"type":"user","message":{"role":"user","content":"<text>"}}` and the same
//!   with `content` as a list of blocks: a block of `text` is the person, and a
//!   block of `tool_result` is no entry.
//! - `{"type":"assistant","message":{"id":"…","content":[{"type":"text","text":"…"},
//!   {"type":"thinking","thinking":"…"},{"type":"tool_use","id":"…","name":"…",
//!   "input":{…}}]}}`: a block of `text` is the agent, a block of `thinking` is no
//!   entry, and a block of `tool_use` is the name of the tool and the first line of
//!   its one summary key.
//! - every other line, such as a line of an `attachment`, of the cost or of the
//!   last prompt, is no entry.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::file::Parsed;
use super::{MAX_SUMMARY_CHARS, MirrorEntry, one_line};

/// The transcript of the session `session` of `home`.
pub(super) fn source(home: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return None;
    }
    let name = format!("{session}.jsonl");
    let entries = std::fs::read_dir(home.join(".claude/projects")).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path().join(&name))
        .find(|path| path.is_file())
}

/// The entries of the lines of one transcript.
pub(super) fn entries(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut last_agent = None;
    let mut message: Option<String> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            parsed.unread += 1;
            continue;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            parsed.unread += 1;
            continue;
        };
        match kind {
            "user" => {
                message = None;
                last_agent = None;
                person(&value, &mut parsed);
            }
            "assistant" => {
                // Two lines of one message hold two parts of its text, so a part
                // joins the entry of the message before it.
                let id = value.pointer("/message/id").and_then(Value::as_str).map(str::to_owned);
                if id != message {
                    last_agent = None;
                }
                message.clone_from(&id);
                last_agent = assistant(&value, &mut parsed, last_agent);
            }
            _ => {}
        }
    }
    parsed
}

/// The blocks of one line of the person.
fn person(value: &Value, parsed: &mut Parsed) {
    // A line that the program marks as its own is no text of the person.
    if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return;
    }
    match value.pointer("/message/content") {
        Some(Value::String(text)) => parsed.person(text),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parsed.person(text);
                }
            }
        }
        _ => {}
    }
}

/// The blocks of one line of the agent: its text, and one tool for each call it
/// made.
fn assistant(value: &Value, parsed: &mut Parsed, mut last_agent: Option<usize>) -> Option<usize> {
    let blocks = value.pointer("/message/content").and_then(Value::as_array)?;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    last_agent = parsed.agent(last_agent, text);
                }
            }
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                parsed.push(MirrorEntry::Tool {
                    name: one_line(name, MAX_SUMMARY_CHARS),
                    request: super::summary(name, block.get("input")),
                });
            }
            _ => {}
        }
    }
    last_agent
}
