//! The sessions of Pi: its list and the entries of one session.
//!
//! Pi writes one conversation in one file under `~/.pi/agent/sessions/<folder>/`,
//! whose `folder` is named after the working directory of the conversation. The
//! record of a session comes from the reader of the session picker of a panel, as
//! the records of Claude Code and Codex do.
//!
//! The shapes of a line that this build reads, measured on the stores of this
//! computer, are:
//!
//! - `{"type":"message","message":{"role":"user","content":[{"type":"text",
//!   "text":"…"}]}}`: the person.
//! - the same with `"role":"assistant"` and a block of `text`: the agent, and a
//!   block of `toolCall` with `name` and `arguments`: the tool and its request.
//!   A block of `thinking` is no entry.
//! - the same with `"role":"toolResult"`: no entry.
//! - every other line, such as the line of the session, of its name or of a change
//!   of the model, is no entry.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::file::Parsed;
use super::{MAX_SUMMARY_CHARS, MAX_TEXT_CHARS, MirrorEntry, MirrorSessionRow, one_line, session_row};

/// The most session files whose names one search reads.
const MAX_FILES: usize = 4_000;

/// The conversation file of the session `session` of `home`.
pub(super) fn source(home: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return None;
    }
    let suffix = format!("{session}.jsonl");
    let entries = std::fs::read_dir(home.join(".pi/agent/sessions")).ok()?;
    let mut seen = 0usize;
    for folder in entries.flatten() {
        let Ok(files) = std::fs::read_dir(folder.path()) else {
            continue;
        };
        for file in files.flatten() {
            seen += 1;
            if seen > MAX_FILES {
                return None;
            }
            let path = file.path();
            if path
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

/// The rows of the Pi sessions of `home`, newest first.
pub(super) fn rows(home: &Path, now_millis: i64) -> Vec<MirrorSessionRow> {
    let oldest = now_millis.saturating_sub(super::MAX_AGE_DAYS.saturating_mul(24 * 60 * 60 * 1_000));
    crate::runtime_state::load_pi_sessions_of_home(home)
        .into_iter()
        .filter(|record| record.interactive && record.updated_at >= oldest)
        .filter_map(|record| {
            let session = one_line(&record.session_id, super::MAX_SESSION_ID_CHARS);
            if session.is_empty() {
                return None;
            }
            Some(MirrorSessionRow {
                source: source(home, &session),
                session: session_row(&record, super::MirrorProgram::Pi, home, false, session),
            })
        })
        .collect()
}

/// The entries of the lines of one conversation.
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
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            parsed.unread += 1;
            continue;
        };
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                for block in blocks(message) {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parsed.push(MirrorEntry::Person {
                            text: one_line(text, MAX_TEXT_CHARS),
                        });
                    }
                }
            }
            Some("assistant") => last_agent = assistant(message, &mut parsed, last_agent),
            _ => {}
        }
    }
    parsed
}

/// The blocks of the content of one message.
fn blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
}

/// The text and the tool calls of one line of the agent.
fn assistant(message: &Value, parsed: &mut Parsed, mut last_agent: Option<usize>) -> Option<usize> {
    for block in blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    last_agent = parsed.agent(last_agent, text);
                }
            }
            Some("toolCall") => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
                parsed.push(MirrorEntry::Tool {
                    name: one_line(name, MAX_SUMMARY_CHARS),
                    request: super::request_text(block.get("arguments").and_then(Value::as_str).unwrap_or_default()),
                });
            }
            _ => {}
        }
    }
    last_agent
}
