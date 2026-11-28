//! The entries of one Codex session, from the rollout file of that session.
//!
//! A rollout is one JSON object on each line, with a `type` and a `payload`. The
//! shapes that this build reads, measured on the rollouts of this computer, are:
//!
//! - `{"type":"response_item","payload":{"type":"message","role":"user","content":
//!   [{"type":"input_text","text":"…"}]}}`: the person.
//! - the same with `"role":"assistant"` and a block of `output_text`: the agent.
//! - `{"type":"response_item","payload":{"type":"function_call","name":"…",
//!   "arguments":"…"}}` and the same with `custom_tool_call` and a text `input`:
//!   the name of the tool and the first line of its request.
//! - every other line, such as a line of `reasoning`, of a result of a tool of the
//!   event log of a turn, is no entry.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::file::Parsed;
use super::{MAX_SUMMARY_CHARS, MAX_TEXT_CHARS, MirrorEntry, one_line};

/// The most session files that a search of one session reads the names of.
const MAX_FILES: usize = 4_000;

/// The rollout of the session `session` of `home`.
pub(super) fn source(home: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return None;
    }
    let suffix = format!("{session}.jsonl");
    let root = home.join(".codex/sessions");
    let mut folders = vec![root];
    let mut seen = 0usize;
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_FILES {
                return None;
            }
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.ends_with(&suffix))
            {
                return Some(path);
            }
        }
    }
    None
}

/// The entries of the lines of one rollout.
pub(super) fn entries(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut last_agent = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            parsed.unread += 1;
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            parsed.unread += 1;
            continue;
        };
        match payload.get("type").and_then(Value::as_str) {
            Some("message") => {
                last_agent = message(payload, &mut parsed, last_agent);
            }
            Some("function_call") => {
                let name = payload.get("name").and_then(Value::as_str).unwrap_or_default();
                parsed.push(tool(name, payload.get("arguments").and_then(Value::as_str)));
            }
            Some("custom_tool_call") => {
                let name = payload.get("name").and_then(Value::as_str).unwrap_or_default();
                parsed.push(tool(name, payload.get("input").and_then(Value::as_str)));
            }
            _ => {}
        }
    }
    parsed
}

/// One message of the person or of the agent.
fn message(payload: &Value, parsed: &mut Parsed, last_agent: Option<usize>) -> Option<usize> {
    let text = texts(payload.get("content"), "input_text", "output_text");
    match payload.get("role").and_then(Value::as_str) {
        Some("user") => {
            if !text.is_empty() {
                parsed.push(MirrorEntry::Person {
                    text: one_line(&text, MAX_TEXT_CHARS),
                });
            }
            None
        }
        Some("assistant") => parsed.agent(last_agent, &text),
        _ => last_agent,
    }
}

/// The text of the blocks of one message: the input blocks of the person beside the
/// output blocks of the agent.
fn texts(content: Option<&Value>, input: &str, output: &str) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some(kind) if kind == input || kind == output
            )
        })
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One tool call of the agent.
fn tool(name: &str, request: Option<&str>) -> MirrorEntry {
    MirrorEntry::Tool {
        name: one_line(name, MAX_SUMMARY_CHARS),
        request: super::request_text(request.unwrap_or_default()),
    }
}
