use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::PanelKind;

const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    #[default]
    Unknown,
    Working,
    Finished,
    Interrupted,
    Blocked,
    Failed,
}

/// Bounded tail evidence without transcript text. A changed length or tail
/// invalidates a saved handoff; malformed/truncated input never proves work.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranscriptSnapshot {
    pub bytes: u64,
    pub tail_sha256: [u8; 32],
    pub state: TurnState,
}

impl TranscriptSnapshot {
    /// Read at most 256 KiB from a provider transcript.
    ///
    /// # Errors
    /// Returns an I/O error if the transcript is missing or changes during the read.
    pub fn read(path: &Path, kind: PanelKind) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let before = file.metadata()?;
        let start = before.len().saturating_sub(TAIL_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut tail = Vec::new();
        (&mut file).take(TAIL_BYTES + 1).read_to_end(&mut tail)?;
        let after = file.metadata()?;
        if before.len() != after.len() || before.modified()? != after.modified()? || tail.len() as u64 > TAIL_BYTES {
            return Err(io::Error::other("transcript changed while being read"));
        }
        let complete_lines = if start == 0 {
            tail.as_slice()
        } else {
            tail.iter()
                .position(|byte| *byte == b'\n')
                .map_or(&[][..], |index| &tail[index + 1..])
        };
        Ok(Self {
            bytes: before.len(),
            tail_sha256: Sha256::digest(&tail).into(),
            state: classify(kind, complete_lines),
        })
    }
}

fn classify(kind: PanelKind, bytes: &[u8]) -> TurnState {
    if bytes.is_empty() || bytes.last() != Some(&b'\n') {
        return TurnState::Unknown;
    }
    let mut state = TurnState::Unknown;
    for line in bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return TurnState::Unknown;
        };
        let next = match kind {
            PanelKind::Claude => claude_state(&value),
            PanelKind::Codex => codex_state(&value),
            _ => return TurnState::Unknown,
        };
        if let Some(next) = next {
            state = next;
        }
    }
    state
}

fn claude_state(value: &Value) -> Option<TurnState> {
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if value.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        return Some(TurnState::Failed);
    }
    let message = value.get("message")?;
    match value.get("type").and_then(Value::as_str)? {
        "assistant" => Some(match message.get("stop_reason").and_then(Value::as_str) {
            Some("end_turn") => TurnState::Finished,
            Some("stop_sequence" | "refusal") => TurnState::Failed,
            Some("tool_use") => {
                if message.get("content").and_then(Value::as_array).is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block.get("name").and_then(Value::as_str) == Some("AskUserQuestion"))
                }) {
                    TurnState::Blocked
                } else {
                    TurnState::Working
                }
            }
            None if message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty()) =>
            {
                TurnState::Working
            }
            _ => TurnState::Unknown,
        }),
        "user" => {
            if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            let content = message.get("content")?;
            let text = content
                .as_str()
                .or_else(|| content.as_array()?.first()?.get("text")?.as_str());
            Some(match text {
                Some(text) if text.starts_with("[Request interrupted by user") => TurnState::Interrupted,
                Some(text) if text.starts_with("<task-notification>") => TurnState::Working,
                Some(text) if text.starts_with('<') || text.starts_with('/') => TurnState::Unknown,
                Some(text) if !text.trim().is_empty() => TurnState::Working,
                None if content.as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                }) =>
                {
                    TurnState::Working
                }
                _ => TurnState::Unknown,
            })
        }
        _ => None,
    }
}

fn codex_state(value: &Value) -> Option<TurnState> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    match payload.get("type").and_then(Value::as_str)? {
        "task_started" => Some(TurnState::Working),
        "task_complete" => Some(TurnState::Finished),
        "turn_aborted" => Some(TurnState::Interrupted),
        "error" => Some(TurnState::Failed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_interrupted_blocked_and_unanswered_turns_are_distinct() {
        for (line, expected) in [
            (
                r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
                TurnState::Finished,
            ),
            (
                r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#,
                TurnState::Interrupted,
            ),
            (
                r#"{"type":"assistant","message":{"stop_reason":"stop_sequence"}}"#,
                TurnState::Failed,
            ),
            (
                r#"{"type":"user","message":{"content":"please work"}}"#,
                TurnState::Working,
            ),
            (
                r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"name":"AskUserQuestion"}]}}"#,
                TurnState::Blocked,
            ),
        ] {
            assert_eq!(classify(PanelKind::Claude, format!("{line}\n").as_bytes()), expected);
        }
    }

    #[test]
    fn unanswered_background_notifications_preserve_work_evidence() {
        let tail =
            b"{\"type\":\"user\",\"message\":{\"content\":\"<task-notification>completed</task-notification>\"}}\n";
        assert_eq!(classify(PanelKind::Claude, tail), TurnState::Working);
    }

    #[test]
    fn unfinished_lines_and_corruption_never_prove_work() {
        for tail in [b"{\"type\":\"user\"}".as_slice(), b"not-json\n", b"", b"{}\n"] {
            assert_eq!(classify(PanelKind::Claude, tail), TurnState::Unknown);
        }
    }

    #[test]
    fn task_completion_and_abort_override_started_event() {
        let start = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
        for (event, expected) in [
            ("task_complete", TurnState::Finished),
            ("turn_aborted", TurnState::Interrupted),
        ] {
            let tail = format!("{start}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{event}\"}}}}\n");
            assert_eq!(classify(PanelKind::Codex, tail.as_bytes()), expected);
        }
    }

    #[test]
    fn snapshot_tracks_tail_advancement_without_retaining_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"private prompt\"}}\n",
        )
        .expect("write");
        let snapshot = TranscriptSnapshot::read(&path, PanelKind::Claude).expect("snapshot");
        assert_eq!(snapshot.state, TurnState::Working);
        assert!(
            !serde_json::to_string(&snapshot)
                .expect("json")
                .contains("private prompt")
        );
        std::fs::write(&path, "{}\n").expect("replace");
        assert_ne!(
            snapshot,
            TranscriptSnapshot::read(&path, PanelKind::Claude).expect("snapshot")
        );
    }
}
