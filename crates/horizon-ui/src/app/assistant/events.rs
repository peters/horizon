//! What a real agent (Claude, Codex) says and does, read from the transcript it writes itself.
//!
//! The terminal is a picture of a program; the transcript is its record. Reading the record gives the
//! feed structured events without parsing the screen: what the person asked, what the agent said, and
//! a short label for each tool it used. Only the tail is followed, and nothing is sent anywhere.
//!
//! Actions on Horizon itself (`agent_panels`) are left out here: the host reports those with more
//! detail when it runs them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use horizon_core::PanelKind;
use serde_json::Value;

/// A transcript this large is followed from its last bytes, not its first.
const START_TAIL_BYTES: u64 = 96 * 1024;
const MAX_READ_BYTES: u64 = 1024 * 1024;
const LABEL_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// What the person asked.
    You(String),
    /// What the agent said.
    Said(String),
    /// Something the agent did, in a few words.
    Did(String),
}

/// One transcript being followed.
pub(super) struct Tail {
    path: PathBuf,
    kind: PanelKind,
    offset: u64,
}

impl Tail {
    pub(super) fn open(path: PathBuf, kind: PanelKind) -> Self {
        let offset = std::fs::metadata(&path).map_or(0, |meta| meta.len().saturating_sub(START_TAIL_BYTES));
        // Starting mid-file, skip the cut line.
        Self {
            path,
            kind,
            offset: if offset == 0 { 0 } else { offset + 1 },
        }
    }

    /// New complete lines since the last call, as events.
    pub(super) fn poll(&mut self) -> Vec<Event> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(length) = file.metadata().map(|meta| meta.len()) else {
            return Vec::new();
        };
        if length < self.offset {
            // Rewritten: start again.
            self.offset = 0;
        }
        if length == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        if file.take(MAX_READ_BYTES).read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            return Vec::new();
        };
        self.offset += u64::try_from(end + 1).unwrap_or(0);
        String::from_utf8_lossy(&bytes[..=end])
            .lines()
            .flat_map(|line| parse_line(self.kind, line))
            .collect()
    }
}

/// Finds the transcript of an agent session: Claude names its file after the session id; Codex
/// writes a rollout file per session whose first line names the working directory.
pub(super) fn locate(
    kind: PanelKind,
    user_home: Option<&Path>,
    codex_home: Option<&Path>,
    session: Option<&str>,
    cwd: Option<&Path>,
    since: SystemTime,
) -> Option<PathBuf> {
    match kind {
        PanelKind::Claude => locate_claude(user_home?, session?),
        PanelKind::Codex => locate_codex(codex_home?, cwd?, since),
        _ => None,
    }
}

fn locate_claude(home: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || !session.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    std::fs::read_dir(home.join(".claude/projects"))
        .ok()?
        .flatten()
        .map(|dir| dir.path().join(format!("{session}.jsonl")))
        .find(|path| path.is_file())
}

fn locate_codex(codex_home: &Path, cwd: &Path, since: SystemTime) -> Option<PathBuf> {
    let mut found: Option<(SystemTime, PathBuf)> = None;
    for path in recent_rollouts(&codex_home.join("sessions")) {
        let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
            continue;
        };
        if modified < since || found.as_ref().is_some_and(|(newest, _)| *newest >= modified) {
            continue;
        }
        if rollout_cwd(&path).is_some_and(|dir| dir == cwd) {
            found = Some((modified, path));
        }
    }
    found.map(|(_, path)| path)
}

/// Rollout files of the last few day folders (`YYYY/MM/DD`), newest folders first.
fn recent_rollouts(root: &Path) -> Vec<PathBuf> {
    let sorted = |dir: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|read| read.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        entries.reverse();
        entries
    };
    let mut files = Vec::new();
    for year in sorted(root).into_iter().take(1) {
        for month in sorted(&year).into_iter().take(2) {
            for day in sorted(&month).into_iter().take(3) {
                files.extend(sorted(&day).into_iter().take(40));
            }
        }
    }
    files
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-"))
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        })
        .collect()
}

fn rollout_cwd(path: &Path) -> Option<PathBuf> {
    let mut head = Vec::new();
    File::open(path).ok()?.take(64 * 1024).read_to_end(&mut head).ok()?;
    let line = head.split(|byte| *byte == b'\n').next()?;
    let value: Value = serde_json::from_slice(line).ok()?;
    value["payload"]["cwd"].as_str().map(PathBuf::from)
}

pub(super) fn parse_line(kind: PanelKind, line: &str) -> Vec<Event> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    match kind {
        PanelKind::Claude => claude(&value),
        PanelKind::Codex => codex(&value),
        _ => Vec::new(),
    }
}

// ---- Claude -------------------------------------------------------------------------------

fn claude(value: &Value) -> Vec<Event> {
    if value["isSidechain"].as_bool().unwrap_or(false) || value["isMeta"].as_bool().unwrap_or(false) {
        return Vec::new();
    }
    let content = &value["message"]["content"];
    match value["type"].as_str() {
        Some("user") => {
            let text = match content {
                Value::String(text) => text.clone(),
                Value::Array(blocks) => blocks
                    .iter()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            person(&text).into_iter().collect()
        }
        Some("assistant") => content
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| match block["type"].as_str() {
                        Some("text") => block["text"].as_str().and_then(agent_text),
                        Some("tool_use") => tool_label(block["name"].as_str().unwrap_or_default(), &block["input"]),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ---- Codex --------------------------------------------------------------------------------

fn codex(value: &Value) -> Vec<Event> {
    let payload = &value["payload"];
    match (value["type"].as_str(), payload["type"].as_str()) {
        (Some("event_msg"), Some("user_message")) => payload["message"].as_str().and_then(person).into_iter().collect(),
        (Some("response_item"), Some("message")) if payload["role"] == "assistant" => payload["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block["type"] == "output_text")
                    .filter_map(|block| block["text"].as_str())
                    .filter_map(agent_text)
                    .collect()
            })
            .unwrap_or_default(),
        (Some("response_item"), Some("function_call")) => {
            let name = payload["name"].as_str().unwrap_or_default();
            // Arguments are a JSON string.
            let arguments: Value = payload["arguments"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            let command = match &arguments["command"] {
                Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
                Value::String(text) => text.clone(),
                _ => String::new(),
            };
            if matches!(name, "shell" | "exec_command" | "local_shell") && !command.is_empty() {
                vec![Event::Did(format!("Ran {}", shorten(first_line(&command))))]
            } else {
                tool_label(name, &arguments).into_iter().collect()
            }
        }
        (Some("response_item"), Some("custom_tool_call")) => {
            let name = payload["name"].as_str().unwrap_or_default();
            if name == "apply_patch" {
                vec![Event::Did("Edited files".to_string())]
            } else {
                tool_label(name, &Value::Null).into_iter().collect()
            }
        }
        _ => Vec::new(),
    }
}

// ---- shared -------------------------------------------------------------------------------

/// What the person typed, if it is something they wrote: commands and injected context are not.
fn person(text: &str) -> Option<Event> {
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:")).then(|| Event::You(text.to_string()))
}

fn agent_text(text: &str) -> Option<Event> {
    let text = text.trim();
    (!text.is_empty()).then(|| Event::Said(text.to_string()))
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

fn shorten(text: &str) -> String {
    if text.chars().count() <= LABEL_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(LABEL_CHARS - 1).collect();
    cut.push('…');
    cut
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A few words for a tool use; `None` for what the host reports itself or is not worth a line.
fn tool_label(name: &str, input: &Value) -> Option<Event> {
    let path = || {
        input["file_path"]
            .as_str()
            .or_else(|| input["path"].as_str())
            .unwrap_or_default()
    };
    let label = match name {
        "Bash" | "shell" => match input["description"].as_str().filter(|text| !text.is_empty()) {
            Some(text) => shorten(first_line(text)),
            None => format!(
                "Ran {}",
                shorten(first_line(input["command"].as_str().unwrap_or_default()))
            ),
        },
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => format!("Edited {}", base_name(path())),
        "Read" => format!("Read {}", base_name(path())),
        "Grep" | "Glob" => "Searched the code".to_string(),
        "WebFetch" | "WebSearch" => "Looked something up".to_string(),
        "Task" | "Agent" => "Handed a part to a helper".to_string(),
        "" | "TodoWrite" | "ToolSearch" | "Skill" => return None,
        other if other.contains("agent_panels") => return None,
        other if other.starts_with("mcp__") => {
            let tool = other.rsplit("__").next().unwrap_or(other);
            format!("Used {}", tool.replace('_', " "))
        }
        other => format!("Used {other}"),
    };
    Some(Event::Did(label))
}

#[cfg(test)]
mod tests;
