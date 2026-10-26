//! Agent requests to run one bounded command in a cloud worker's agent account, and the
//! private journal of what Horizon asked the worker to do. The host authorizes the cloud
//! against its own records and the worker decides the account, so no field here names a
//! user, a uid or an environment.
use super::audit;
use super::now_millis;
use crate::paths::{BrowserRuntimePaths, safe_local_id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// The longest command text an agent may send, in bytes. `ssh` carries it as one argv word.
pub const MAX_COMMAND_BYTES: usize = 8 * 1024;
/// The wall-clock bound of one command, in milliseconds.
pub const MIN_TIMEOUT_MILLIS: u64 = 1_000;
pub const MAX_TIMEOUT_MILLIS: u64 = 120_000;
pub const DEFAULT_TIMEOUT_MILLIS: u64 = 30_000;
/// The extra time the host's answer takes beyond the command's own bound: the route to the
/// worker, Horizon's recorded readiness check and the reply.
pub const ANSWER_HEADROOM_MILLIS: i64 = 30_000;

fn default_timeout_millis() -> u64 {
    DEFAULT_TIMEOUT_MILLIS
}

/// One command for one cloud's worker. An unknown field, including a `user`, a `uid` or an
/// `env`, fails to decode, so a request that asks for another account never reaches a host.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudExecRequest {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// One shell command text, which the worker runs with `/bin/sh -c` as its agent user.
    pub command: String,
    /// The wall-clock bound of the command between 1000 and 120000 ms.
    #[serde(default = "default_timeout_millis")]
    pub timeout_millis: u64,
}

impl CloudExecRequest {
    /// Whether the fields are present and bounded. The host still authorizes the cloud
    /// against its own records and stops a command that switches user.
    #[must_use]
    pub fn valid(&self) -> bool {
        let cloud = !self.cloud.is_empty()
            && self.cloud.len() <= 128
            && self
                .cloud
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'));
        cloud
            && !self.command.trim().is_empty()
            && self.command.len() <= MAX_COMMAND_BYTES
            && !self.command.contains('\0')
            && (MIN_TIMEOUT_MILLIS..=MAX_TIMEOUT_MILLIS).contains(&self.timeout_millis)
    }

    /// The time the host has to answer, from the moment the request is queued.
    #[must_use]
    pub fn answer_millis(&self) -> i64 {
        i64::try_from(self.timeout_millis)
            .unwrap_or(i64::MAX)
            .saturating_add(ANSWER_HEADROOM_MILLIS)
    }
}

/// The journal's outcome of one call.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudExecOutcome {
    /// The worker ran the command and reported its exit status, zero or not.
    Completed,
    /// Horizon refused to run the command, or could not reach the worker.
    Failed,
    /// The command outlived its bound and Horizon stopped it.
    TimedOut,
}

/// One line of the private cloud command journal.
///
/// It records what Horizon asked for and how it ended, never the command text: a command
/// can carry a token the caller pasted into it, so the journal keeps the length and a
/// SHA-256 digest, which is enough to compare two calls and never enough to replay one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudExecAudit {
    pub schema_version: u32,
    pub event_id: String,
    pub at_millis: i64,
    /// The calling agent's actor, as the queue recorded it.
    pub actor: String,
    pub cloud: String,
    pub command_bytes: u64,
    pub command_sha256: String,
    pub outcome: CloudExecOutcome,
    /// The typed code of a refusal or failure, or `None` when the command ran.
    pub code: Option<String>,
    /// The exit status the worker reported, when it reported one.
    pub exit_status: Option<i32>,
    /// The account the command ran as, when the worker proved it.
    pub uid: Option<u32>,
    pub duration_millis: Option<u64>,
}

impl CloudExecAudit {
    /// A record of one call that has not finished yet.
    #[must_use]
    pub fn queued(actor: &str, request: &CloudExecRequest) -> Self {
        Self {
            schema_version: 1,
            event_id: horizon_browser::new_action_id(),
            at_millis: now_millis(),
            actor: actor.to_owned(),
            cloud: request.cloud.clone(),
            command_bytes: u64::try_from(request.command.len()).unwrap_or(u64::MAX),
            command_sha256: command_sha256(&request.command),
            outcome: CloudExecOutcome::Failed,
            code: None,
            exit_status: None,
            uid: None,
            duration_millis: None,
        }
    }
}

/// The digest of a command text, so two calls can be compared without keeping the text.
#[must_use]
pub fn command_sha256(command: &str) -> String {
    Sha256::digest(command.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut digest, byte| {
            use std::fmt::Write as _;
            let _ = write!(digest, "{byte:02x}");
            digest
        })
}

#[must_use]
pub fn cloud_exec_audit_path(cloud: &str) -> PathBuf {
    BrowserRuntimePaths::resolve()
        .root()
        .join("audit")
        .join("cloud-exec")
        .join(format!("{}.jsonl", safe_local_id(cloud)))
}

/// Appends one journal line. The journal is private, bounded and rotated as the browser
/// audit journal is; a failed append changes nothing that the caller already answered.
///
/// # Errors
/// Unavailable or unwritable private storage.
pub fn append_cloud_exec_audit(record: &CloudExecAudit) -> std::io::Result<()> {
    audit::append_record_at(&cloud_exec_audit_path(&record.cloud), record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: &str, timeout_millis: u64) -> CloudExecRequest {
        CloudExecRequest {
            cloud: "cloud-1".into(),
            command: command.into(),
            timeout_millis,
        }
    }

    #[test]
    fn only_a_bounded_command_for_a_cloud_identifier_is_valid() {
        assert!(request("git commit -am 'work'", DEFAULT_TIMEOUT_MILLIS).valid());
        assert!(request("  git status\n", MIN_TIMEOUT_MILLIS).valid());
        for invalid in [
            request("   ", DEFAULT_TIMEOUT_MILLIS),
            request("", DEFAULT_TIMEOUT_MILLIS),
            request(&"a".repeat(MAX_COMMAND_BYTES + 1), DEFAULT_TIMEOUT_MILLIS),
            request("git\0status", DEFAULT_TIMEOUT_MILLIS),
            request("git status", MIN_TIMEOUT_MILLIS - 1),
            request("git status", MAX_TIMEOUT_MILLIS + 1),
        ] {
            assert!(!invalid.valid(), "{invalid:?}");
        }
        for cloud in ["", "a/b", &"a".repeat(129)] {
            assert!(
                !CloudExecRequest {
                    cloud: cloud.into(),
                    ..request("git status", DEFAULT_TIMEOUT_MILLIS)
                }
                .valid()
            );
        }
    }

    #[test]
    fn a_request_that_names_a_user_or_an_environment_does_not_decode() {
        for field in ["user", "uid", "env", "shell"] {
            let mut value = serde_json::json!({"cloud": "cloud-1", "command": "git status"});
            value[field] = serde_json::json!("root");
            assert!(
                serde_json::from_value::<CloudExecRequest>(value).is_err(),
                "{field} must not decode"
            );
        }
        let decoded =
            serde_json::from_value::<CloudExecRequest>(serde_json::json!({"cloud": "cloud-1", "command": "true"}))
                .unwrap();
        assert_eq!(decoded.timeout_millis, DEFAULT_TIMEOUT_MILLIS);
    }

    #[test]
    fn the_journal_keeps_the_shape_of_a_command_and_never_its_text() {
        let request = request("gh pr merge 12 --squash --body 'token ghp_secret'", 5_000);
        let record = CloudExecAudit::queued("horizon:agent", &request);
        assert_eq!(record.command_bytes, request.command.len() as u64);
        assert_eq!(record.command_sha256, command_sha256(&request.command));
        assert_eq!(record.command_sha256.len(), 64);
        assert_ne!(record.command_sha256, command_sha256("gh pr merge 12 --squash"));
        assert_eq!(request.answer_millis(), 35_000);
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(!encoded.contains("ghp_secret"), "{encoded}");
        assert!(!encoded.contains("gh pr merge"), "{encoded}");
    }
}
