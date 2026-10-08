//! Requests agents make for more GitHub access, and the person's decisions.
//!
//! An agent asks with the worker's `github_access` tool. The worker keeps the request;
//! Horizon lists it on the cloud card, and the person allows it for the task, for the
//! cloud, or denies it. The worker checks that the repository is reachable before it
//! allows anything, and enforces the grant on every credential answer.
use super::{Result, Runner};
use crate::cloud_runtime::ssh::Connection;
use serde::Deserialize;
use std::time::Duration;

const LIST: &str = "if command -v horizon-worker-github >/dev/null 2>&1; then horizon-worker-github requests; \
     else printf '{\"requests\":[]}'; fi";

/// One pending request.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    /// GitHub `owner/name`.
    pub repository: String,
    /// `push` or `read`.
    pub access: String,
    pub reason: String,
    /// The agent session that asked, as the worker names it.
    #[serde(default)]
    pub session: String,
    /// The agent that asked, such as `claude`.
    #[serde(default)]
    pub agent: String,
}

/// What the person chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    AllowTask,
    AllowCloud,
    Deny,
}

impl Decision {
    const fn argument(self) -> &'static str {
        match self {
            Self::AllowTask => "allow-task",
            Self::AllowCloud => "allow-cloud",
            Self::Deny => "deny",
        }
    }
}

/// The worker's pending requests. Malformed entries are left out.
/// # Errors
/// The worker could not be reached.
pub fn list(connection: &Connection, runner: &Runner<'_>) -> Result<Vec<Request>> {
    let output = runner.run(
        "GitHub access requests",
        &mut connection.command(LIST),
        Duration::from_secs(20),
    )?;
    Ok(parse_list(&output))
}

pub(super) fn parse_list(output: &str) -> Vec<Request> {
    #[derive(Deserialize)]
    struct Fields {
        #[serde(default)]
        requests: Vec<serde_json::Value>,
    }
    let Some(fields) = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Fields>(line.trim()).ok())
    else {
        return Vec::new();
    };
    fields
        .requests
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Request>(value).ok())
        .filter(|request| {
            valid_id(&request.id)
                && horizon_cloud::github::valid_repository(&request.repository)
                && matches!(request.access.as_str(), "push" | "read")
                && request.reason.len() <= 300
                && !request.reason.chars().any(char::is_control)
                && [&request.session, &request.agent]
                    .iter()
                    .all(|value| value.len() <= 100 && !value.chars().any(char::is_control))
        })
        .take(32)
        .collect()
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Sends the person's decision. Returns why the worker did not apply it, such as a
/// repository the app is not installed on, or `None` when it did.
/// # Errors
/// A malformed request ID or a worker that could not be reached.
pub fn decide(connection: &Connection, runner: &Runner<'_>, id: &str, decision: Decision) -> Result<Option<String>> {
    if !valid_id(id) {
        return Err(super::Error::Invalid("Invalid GitHub access request"));
    }
    // The worker reports a refused decision as JSON with a nonzero exit; keep its answer.
    let command = format!(
        "horizon-worker-github decide {id} {} 2>/dev/null; true",
        decision.argument()
    );
    let output = runner.run(
        "GitHub access decision",
        &mut connection.command(&command),
        Duration::from_secs(30),
    )?;
    Ok(parse_decision(&output))
}

pub(super) fn parse_decision(output: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Fields {
        ok: bool,
        #[serde(default)]
        error: Option<String>,
    }
    let fields = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Fields>(line.trim()).ok());
    match fields {
        Some(Fields { ok: true, .. }) => None,
        Some(Fields { error: Some(error), .. }) => Some(explain(&error)),
        _ => Some("The worker did not confirm the decision.".into()),
    }
}

fn explain(code: &str) -> String {
    match code {
        "not_installed" => {
            "The GitHub App is not installed on this repository. Add it in Cloud settings › GitHub, then allow again."
                .into()
        }
        "no_push" => "Your GitHub account cannot push to this repository.".into(),
        "session_ended" | "not_pending" | "unknown_request" => "This request is no longer waiting.".into(),
        "no_chain" | "token_expired" | "token_invalid" => {
            "This cloud has no current GitHub access. Connect GitHub again on the cloud card.".into()
        }
        "forbidden" => "GitHub refused to show this repository to the app.".into(),
        "unreachable" => "The worker could not reach GitHub. Try again.".into(),
        code if code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') && code.len() <= 64 => {
            format!("The worker refused the decision ({code}).")
        }
        _ => "The worker refused the decision.".into(),
    }
}
