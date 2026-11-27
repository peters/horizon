//! The line format of one agent program: the line that a turn writes, and the
//! events that a line of the program becomes.
//!
//! [`ClaudeCodec`] is the codec of Claude Code (`claude -p --input-format
//! stream-json --output-format stream-json --verbose`), measured on version
//! 2.1.332. Every line is one JSON object with a `type` string. A line that is
//! not JSON, and a JSON line without a `type` string, is dropped whole with one
//! [`DropReason`]. A type that this slice does not use (`stream_event`,
//! `system/status`, `system/thinking_tokens`, `system/hook_started`,
//! `system/hook_response`, and every type of a later version) gives nothing, so a
//! new line of the program never costs the conversation an event.
//!
//! The memory of a codec is bounded: it joins the texts of the lines that share
//! one `message.id`, keeps at most [`MAX_TEXT_BYTES`] of one message and at most
//! [`MAX_TRACKED_MESSAGES`] messages, so a program cannot grow this process
//! without a bound.

use std::collections::HashMap;

use serde_json::Value;
use serde_json::json;

use super::event::{BridgeEvent, DropReason};
use super::{MAX_SUMMARY_CHARS, MAX_TEXT_BYTES};

/// The messages whose text one codec joins. A full map is emptied rather than
/// grown; a message id that the map lost starts a new text, which the
/// conversation replaces as before.
const MAX_TRACKED_MESSAGES: usize = 256;

/// The keys of a tool input that become the summary of a tool call, and nothing
/// else of the input is read.
const SUMMARY_KEYS: [(&str, &str); 6] = [
    ("Bash", "command"),
    ("Read", "file_path"),
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("NotebookEdit", "file_path"),
];

/// The codec of one program: the lines that Horizon writes and the events that
/// the lines of the program become.
pub trait LineCodec: Send {
    /// One line for the standard input of the program, without a newline.
    fn user_line(&self, text: &str) -> String;
    /// The events of one line of the program, without its newline.
    fn parse(&mut self, line: &str) -> Vec<BridgeEvent>;
}

/// The codec of Claude Code.
#[derive(Debug, Default)]
pub struct ClaudeCodec {
    texts: HashMap<String, String>,
}

impl ClaudeCodec {
    /// A codec that joins no text yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The events of one `assistant` line: the whole text of its message, and
    /// one start for each tool that it called.
    fn assistant(&mut self, value: &Value) -> Vec<BridgeEvent> {
        let Some(message) = value.get("message") else {
            return vec![dropped(DropReason::UnknownShape)];
        };
        let Some(message_id) = message.get("id").and_then(Value::as_str) else {
            return vec![dropped(DropReason::UnknownShape)];
        };
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            return vec![dropped(DropReason::UnknownShape)];
        };
        let mut events = Vec::new();
        let mut text = String::new();
        let mut tools = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => text.push_str(block.get("text").and_then(Value::as_str).unwrap_or_default()),
                Some("tool_use") => {
                    if let (Some(tool_id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) {
                        tools.push((
                            tool_id.to_owned(),
                            tidy(name, MAX_SUMMARY_CHARS),
                            summary(name, block.get("input")),
                        ));
                    }
                }
                _ => {}
            }
        }
        if !text.is_empty() {
            let whole = self.join(message_id, &text);
            events.push(BridgeEvent::AgentText {
                message_id: message_id.to_owned(),
                text: whole,
            });
        }
        for (tool_id, name, summary) in tools {
            events.push(BridgeEvent::ToolStarted { tool_id, name, summary });
        }
        events
    }

    /// The whole text of one message, which several lines of the program hold.
    fn join(&mut self, message_id: &str, part: &str) -> String {
        let full = self.texts.len() >= MAX_TRACKED_MESSAGES && !self.texts.contains_key(message_id);
        if full {
            self.texts.clear();
        }
        let text = self.texts.entry(message_id.to_owned()).or_default();
        text.push_str(part);
        let keep = floor_boundary(text, MAX_TEXT_BYTES);
        text.truncate(keep);
        text.clone()
    }
}

impl LineCodec for ClaudeCodec {
    fn user_line(&self, text: &str) -> String {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": text }],
            },
        })
        .to_string()
    }

    fn parse(&mut self, line: &str) -> Vec<BridgeEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return vec![dropped(DropReason::NotJson)];
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return vec![dropped(DropReason::UnknownShape)];
        };
        match kind {
            "system" => system(&value),
            "assistant" => self.assistant(&value),
            "user" => tool_results(&value),
            "rate_limit_event" => limit(&value),
            "result" => turn_ended(&value),
            _ => Vec::new(),
        }
    }
}

/// The events of one `system` line.
fn system(value: &Value) -> Vec<BridgeEvent> {
    match value.get("subtype").and_then(Value::as_str) {
        Some("init") => match value.get("session_id").and_then(Value::as_str) {
            Some(session_id) => vec![
                BridgeEvent::SessionStarted {
                    session_id: session_id.to_owned(),
                },
                BridgeEvent::TurnStarted,
            ],
            None => vec![dropped(DropReason::UnknownShape)],
        },
        Some("permission_denied") => match value.get("tool_name").and_then(Value::as_str) {
            Some(tool) => {
                let message = value.get("message").and_then(Value::as_str).unwrap_or_default();
                vec![BridgeEvent::PermissionDenied {
                    tool: tidy(tool, MAX_SUMMARY_CHARS),
                    message: tidy(message, MAX_SUMMARY_CHARS),
                }]
            }
            None => vec![dropped(DropReason::UnknownShape)],
        },
        _ => Vec::new(),
    }
}

/// One refusal.
fn dropped(reason: DropReason) -> BridgeEvent {
    BridgeEvent::Dropped { reason }
}

/// The events of one `user` line, which holds the results of the tool calls that
/// the agent made.
fn tool_results(value: &Value) -> Vec<BridgeEvent> {
    let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(tool_id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        events.push(BridgeEvent::ToolEnded {
            tool_id: tool_id.to_owned(),
            failed: block.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            summary: first_line(&result_text(block.get("content"))),
        });
    }
    events
}

/// The text of one tool result: a string, or the text of its blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The events of one `rate_limit_event` line.
fn limit(value: &Value) -> Vec<BridgeEvent> {
    let Some(info) = value.get("rate_limit_info") else {
        return vec![dropped(DropReason::UnknownShape)];
    };
    let status = info.get("status").and_then(Value::as_str).unwrap_or_default();
    vec![BridgeEvent::Limit {
        status: tidy(status, MAX_SUMMARY_CHARS),
        using_overage: info.get("isUsingOverage").and_then(Value::as_bool).unwrap_or(false),
        resets_at_unix: info.get("resetsAt").and_then(Value::as_u64),
    }]
}

/// The events of one `result` line, which ends a turn. Its `total_cost_usd` is
/// the cost of the whole process, so the cost of this turn is the sum of the
/// `costUSD` of the models of `model_usage_since_last_result`.
fn turn_ended(value: &Value) -> Vec<BridgeEvent> {
    let cost_usd = value
        .get("model_usage_since_last_result")
        .and_then(Value::as_object)
        .filter(|models| !models.is_empty())
        .map(|models| {
            models
                .values()
                .filter_map(|model| model.get("costUSD").and_then(Value::as_f64))
                .sum()
        });
    vec![BridgeEvent::TurnEnded {
        failed: value.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        duration_ms: value.get("duration_ms").and_then(Value::as_u64),
        cost_usd,
    }]
}

/// The summary of one tool call, from its one fixed key and nothing else. A tool
/// without such a key is its own summary.
fn summary(name: &str, input: Option<&Value>) -> String {
    let Some((_, key)) = SUMMARY_KEYS.iter().find(|(tool, _)| *tool == name) else {
        return tidy(name, MAX_SUMMARY_CHARS);
    };
    match input.and_then(|input| input.get(*key)).and_then(Value::as_str) {
        Some(value) => tidy(value, MAX_SUMMARY_CHARS),
        None => tidy(name, MAX_SUMMARY_CHARS),
    }
}

/// The first line of a tool result, cleaned and cut.
fn first_line(text: &str) -> String {
    tidy(text.split('\n').next().unwrap_or_default(), MAX_SUMMARY_CHARS)
}

/// `text` with every control character replaced by one space, cut at `limit`
/// characters.
pub(super) fn tidy(text: &str, limit: usize) -> String {
    text.chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(limit)
        .collect()
}

/// The largest index of `text` at or below `limit` that splits no character.
pub(super) fn floor_boundary(text: &str, limit: usize) -> usize {
    if text.len() <= limit {
        return text.len();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// The first `limit` bytes of `text` that hold whole characters.
pub(super) fn cut_bytes(text: &str, limit: usize) -> &str {
    &text[..floor_boundary(text, limit)]
}
