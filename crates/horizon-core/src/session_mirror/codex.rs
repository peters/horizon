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
use super::{MAX_SUMMARY_CHARS, MirrorEntry, one_line};

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
        // The folders of the store are the year, the month and the day of the
        // start of a session. The names are read in their order, so the folder
        // that the search opens next is the newest one: a session of the list
        // is a new session, and a store of many thousand files does not end the
        // search before it reaches that session. A real store of eleven
        // thousand files did so.
        let mut names: Vec<(std::ffi::OsString, bool)> = entries
            .flatten()
            .map(|entry| {
                let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
                (entry.file_name(), folder)
            })
            .collect();
        names.sort();
        for (name, is_folder) in names {
            seen += 1;
            if seen > MAX_FILES {
                return None;
            }
            if is_folder {
                folders.push(folder.join(name));
            } else if name.to_str().is_some_and(|name| name.ends_with(&suffix)) {
                return Some(folder.join(name));
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
    match payload.get("role").and_then(Value::as_str) {
        Some("user") => {
            // Codex sends the instruction files of a folder and the context of a
            // session as blocks of the first message of the person. Each block is
            // judged alone, so the words of the person do not stand in one text
            // with what the program wrote before them.
            for text in blocks_text(payload.get("content"), "input_text") {
                parsed.person(text);
            }
            None
        }
        Some("assistant") => {
            let text = blocks_text(payload.get("content"), "output_text")
                .collect::<Vec<_>>()
                .join("\n");
            parsed.agent(last_agent, &text)
        }
        _ => last_agent,
    }
}

/// The texts of the blocks of the kind `kind` of one message, in their order.
fn blocks_text<'a>(content: Option<&'a Value>, kind: &'a str) -> impl Iterator<Item = &'a str> {
    content
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(move |block| block.get("type").and_then(Value::as_str) == Some(kind))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
}

/// One tool call of the agent.
fn tool(name: &str, request: Option<&str>) -> MirrorEntry {
    let request = request.unwrap_or_default();
    // The arguments of a function call are a document in a text. The request is
    // the one key of the tool when the document holds it, and the first line of
    // the text otherwise.
    let of_key = serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|input| super::key_request(name, &input));
    MirrorEntry::Tool {
        name: one_line(name, MAX_SUMMARY_CHARS),
        request: of_key.unwrap_or_else(|| super::request_text(request)),
    }
}
