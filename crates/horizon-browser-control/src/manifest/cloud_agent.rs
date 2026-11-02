//! Agent requests to type one instruction into a cloud worker's agent terminal, and to
//! read the bounded tail of what that terminal shows. The host resolves the session from
//! the cloud's own record, so no field here names a process, a user or a path. The journal
//! keeps the shape of a write and never its text, because an instruction can name what the
//! person may not want stored.
use super::audit;
use super::now_millis;
use crate::paths::{BrowserRuntimePaths, safe_local_id};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The most instruction text one write carries, in bytes. One instruction is one line: a
/// newline would submit a part of it before the rest arrives.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024;
/// The fewest and most lines of a terminal a read asks for.
pub const MIN_READ_LINES: u32 = 20;
pub const MAX_READ_LINES: u32 = 600;
pub const DEFAULT_READ_LINES: u32 = 120;
/// The most terminal text one read answer carries, in bytes. A longer tail is cut at its
/// start, so the newest output always arrives whole.
pub const MAX_READ_BYTES: usize = 16 * 1024;
/// The lines of the terminal that a write answers with, so the caller sees the session take
/// the instruction in the same call.
pub const CONFIRM_LINES: u32 = 40;
/// The wall-clock bound of one write or read. Neither is a long job: the route, one tmux
/// command and the reply.
pub const TIMEOUT_MILLIS: u64 = 20_000;
/// The extra time the host's answer takes beyond the call's own bound.
pub const ANSWER_HEADROOM_MILLIS: i64 = 30_000;

/// One instruction for one session of one cloud's worker.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudAgentMessageRequest {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// The panel ID of the session, from `cloud_list`'s `sessions`.
    pub session: String,
    /// One instruction, as plain printable text.
    pub text: String,
}

impl CloudAgentMessageRequest {
    /// Whether the fields are present and bounded. The host still resolves the session
    /// against the cloud's own record, and proves that its worker runs it.
    #[must_use]
    pub fn valid(&self) -> bool {
        cloud(&self.cloud) && session(&self.session) && text(&self.text)
    }

    /// The time the host has to answer, from the moment the request is queued.
    #[must_use]
    pub fn answer_millis(&self) -> i64 {
        i64::try_from(TIMEOUT_MILLIS)
            .unwrap_or(i64::MAX)
            .saturating_add(ANSWER_HEADROOM_MILLIS)
    }
}

/// One read of one session's terminal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudAgentReadRequest {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// The panel ID of the session, from `cloud_list`'s `sessions`.
    pub session: String,
    /// How many lines back from the visible screen to start, 20 to 600.
    #[serde(default = "default_read_lines")]
    pub lines: u32,
}

fn default_read_lines() -> u32 {
    DEFAULT_READ_LINES
}

impl CloudAgentReadRequest {
    /// Whether the fields are present and bounded, as [`CloudAgentMessageRequest::valid`].
    #[must_use]
    pub fn valid(&self) -> bool {
        cloud(&self.cloud) && session(&self.session) && (MIN_READ_LINES..=MAX_READ_LINES).contains(&self.lines)
    }

    /// The time the host has to answer, from the moment the request is queued.
    #[must_use]
    pub fn answer_millis(&self) -> i64 {
        CloudAgentMessageRequest {
            cloud: self.cloud.clone(),
            session: self.session.clone(),
            text: String::new(),
        }
        .answer_millis()
    }
}

/// What one call did, as the private journal records it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudAgentAction {
    /// One instruction was typed into the session's terminal.
    Message,
    /// The tail of the session's terminal was read.
    Read,
}

/// The journal's outcome of one call.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudAgentOutcome {
    /// The worker ran the tmux command and answered.
    Completed,
    /// Horizon refused the call, or could not reach the worker.
    Failed,
}

/// One line of the private cloud agent journal. It holds the cloud, the session, the
/// action and the byte counts, and never the instruction text.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudAgentAudit {
    pub schema_version: u32,
    pub event_id: String,
    pub at_millis: i64,
    /// The calling agent's actor, as the queue recorded it.
    pub actor: String,
    pub cloud: String,
    pub session: String,
    pub action: CloudAgentAction,
    /// The instruction's byte length, which is zero for a read.
    pub text_bytes: u64,
    /// The terminal text the answer carried, which is zero for a write without a tail.
    pub answer_bytes: u64,
    pub outcome: CloudAgentOutcome,
    /// The typed code of a refusal or failure, or `None` when the call ran.
    pub code: Option<String>,
    pub exit_status: Option<i32>,
    pub duration_millis: Option<u64>,
}

impl CloudAgentAudit {
    /// A record of one call that has not finished yet.
    #[must_use]
    pub fn queued(actor: &str, cloud: &str, session: &str, action: CloudAgentAction, text_bytes: usize) -> Self {
        Self {
            schema_version: 1,
            event_id: horizon_browser::new_action_id(),
            at_millis: now_millis(),
            actor: actor.to_owned(),
            cloud: cloud.to_owned(),
            session: session.to_owned(),
            action,
            text_bytes: u64::try_from(text_bytes).unwrap_or(u64::MAX),
            answer_bytes: 0,
            outcome: CloudAgentOutcome::Failed,
            code: None,
            exit_status: None,
            duration_millis: None,
        }
    }
}

#[must_use]
pub fn cloud_agent_audit_path(cloud: &str) -> PathBuf {
    BrowserRuntimePaths::resolve()
        .root()
        .join("audit")
        .join("cloud-agent")
        .join(format!("{}.jsonl", safe_local_id(cloud)))
}

/// Appends one journal line, which is private and bounded as the cloud command journal is.
///
/// # Errors
/// Unavailable or unwritable private storage.
pub fn append_cloud_agent_audit(record: &CloudAgentAudit) -> std::io::Result<()> {
    audit::append_record_at(&cloud_agent_audit_path(&record.cloud), record)
}

/// A cloud ID, as `cloud_list` reports it.
fn cloud(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
}

/// A session identifier, as the worker accepts it in `horizon-worker-session` and names its
/// tmux session: no space, no quote and no shell metacharacter.
fn session(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

/// One printable line of instruction text. A control character is refused because the
/// terminal would read it as a key: a carriage return submits early, and an escape leaves
/// the agent's input mode.
fn text(value: &str) -> bool {
    // `is_control` covers the Cc category, which holds every key a terminal would read as
    // something other than itself: a carriage return, a newline, a tab, an escape and DEL.
    !value.trim().is_empty() && value.len() <= MAX_MESSAGE_BYTES && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> CloudAgentMessageRequest {
        CloudAgentMessageRequest {
            cloud: "cloud-1".into(),
            session: "panel-1".into(),
            text: text.into(),
        }
    }

    #[test]
    fn one_printable_instruction_for_one_session_is_valid() {
        assert!(message("read #1774 and execute PR 1").valid());
        assert!(message("  fix the failing check — then push  ").valid());
        for invalid in [
            String::new(),
            "   \n".to_owned(),
            "read it\npush it".to_owned(),
            "press\r".to_owned(),
            "tab\there".to_owned(),
            "escape\u{1b}[A".to_owned(),
            "nul\0byte".to_owned(),
            "a".repeat(MAX_MESSAGE_BYTES + 1),
        ] {
            assert!(!message(&invalid).valid(), "{invalid:?}");
        }
    }

    #[test]
    fn a_cloud_and_a_session_use_the_identifiers_the_worker_accepts() {
        for cloud in ["", "a/b", &"a".repeat(129), "a b"] {
            assert!(
                !CloudAgentMessageRequest {
                    cloud: cloud.into(),
                    ..message("go")
                }
                .valid(),
                "{cloud}"
            );
        }
        for session in ["", "a/b", "a b", &"a".repeat(101), "a'b", "a;b"] {
            assert!(
                !CloudAgentMessageRequest {
                    session: session.into(),
                    ..message("go")
                }
                .valid(),
                "{session}"
            );
        }
    }

    #[test]
    fn a_read_asks_for_a_bounded_number_of_lines_and_defaults_to_one() {
        let read = |lines: u32| CloudAgentReadRequest {
            cloud: "cloud-1".into(),
            session: "panel-1".into(),
            lines,
        };
        assert!(read(DEFAULT_READ_LINES).valid());
        assert!(read(MIN_READ_LINES).valid());
        assert!(read(MAX_READ_LINES).valid());
        for lines in [0, MIN_READ_LINES - 1, MAX_READ_LINES + 1] {
            assert!(!read(lines).valid(), "{lines}");
        }
        let decoded = serde_json::from_value::<CloudAgentReadRequest>(serde_json::json!({
            "cloud": "cloud-1", "session": "panel-1",
        }))
        .unwrap();
        assert_eq!(decoded.lines, DEFAULT_READ_LINES);
        assert_eq!(decoded.answer_millis(), message("go").answer_millis());
    }

    #[test]
    fn a_request_that_names_a_process_or_a_path_does_not_decode() {
        for field in ["command", "tmux", "path", "user", "attach"] {
            let mut value = serde_json::json!({"cloud": "cloud-1", "session": "panel-1", "text": "go"});
            value[field] = serde_json::json!("x");
            assert!(
                serde_json::from_value::<CloudAgentMessageRequest>(value).is_err(),
                "{field} must not decode"
            );
        }
    }

    #[test]
    fn the_journal_keeps_the_shape_of_a_write_and_never_its_text() {
        let request = message("deploy with token ghp_secret");
        let record = CloudAgentAudit::queued(
            "horizon:agent",
            &request.cloud,
            &request.session,
            CloudAgentAction::Message,
            request.text.len(),
        );
        assert_eq!(record.text_bytes, request.text.len() as u64);
        assert_eq!(record.action, CloudAgentAction::Message);
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(!encoded.contains("ghp_secret"), "{encoded}");
        assert!(!encoded.contains("deploy with"), "{encoded}");
        assert_eq!(
            cloud_agent_audit_path("cloud-1"),
            cloud_agent_audit_path("cloud-1"),
            "one journal per cloud"
        );
        assert_ne!(cloud_agent_audit_path("cloud-1"), cloud_agent_audit_path("cloud-2"));
        assert_eq!(
            cloud_agent_audit_path("cloud-1")
                .parent()
                .and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("cloud-agent"))
        );
    }
}
