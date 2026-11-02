//! Agent requests to type into a cloud session's agent terminal and to read it back. Horizon
//! resolves the cloud and the session from its own records, then answers from a background
//! thread: neither the route to the worker nor a blocked write runs on the UI thread.
use super::HorizonApp;
use super::route::{Codes, Route};
use horizon_core::{
    browser::manifest::provider_usage::{
        CloudAgentAction, CloudAgentAudit, CloudAgentMessageRequest, CloudAgentOutcome, CloudAgentReadRequest,
        UsageRequest, append_cloud_agent_audit, complete_provider_usage,
    },
    cloud_runtime::{
        Cancellation,
        agent_session::{self, Failure, Outcome},
        cloud_exec::Target,
        state::{Deployment, Session, Store},
    },
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The codes and sentences of each tool, so a caller reads the code of the call it made.
const MESSAGE_CODES: Codes = Codes {
    prefix: "cloud_agent",
    request: "one instruction of printable text for one cloud and session from cloud_list",
    started: "the write",
};

const READ_CODES: Codes = Codes {
    prefix: "cloud_agent",
    request: "one read of one cloud and session from cloud_list",
    started: "the read",
};

/// What one request asked, once the host has read it.
enum Ask {
    Message(CloudAgentMessageRequest),
    Read(CloudAgentReadRequest),
}

impl Ask {
    fn of(request: &UsageRequest) -> Result<Self, String> {
        if let Some(message) = request.cloud_agent_message.as_ref().filter(|message| message.valid()) {
            return Ok(Self::Message(message.clone()));
        }
        if let Some(read) = request.cloud_agent_read.as_ref().filter(|read| read.valid()) {
            return Ok(Self::Read(read.clone()));
        }
        Err("cloud_agent_invalid_request: one instruction of printable text, or one read of 20 to 600 lines, for one cloud and session from cloud_list".to_owned())
    }

    const fn codes(&self) -> &'static Codes {
        match self {
            Self::Message(_) => &MESSAGE_CODES,
            Self::Read(_) => &READ_CODES,
        }
    }

    fn cloud(&self) -> &str {
        match self {
            Self::Message(message) => &message.cloud,
            Self::Read(read) => &read.cloud,
        }
    }

    fn session(&self) -> &str {
        match self {
            Self::Message(message) => &message.session,
            Self::Read(read) => &read.session,
        }
    }

    fn action(&self) -> CloudAgentAction {
        match self {
            Self::Message(_) => CloudAgentAction::Message,
            Self::Read(_) => CloudAgentAction::Read,
        }
    }

    fn text_bytes(&self) -> usize {
        match self {
            Self::Message(message) => message.text.len(),
            Self::Read(_) => 0,
        }
    }

    /// The session's own record, which names the terminal the worker runs.
    fn record(route: &Route) -> Result<Deployment, String> {
        Store::snapshot(&route.directory)
            .map_err(|error| format!("cloud_agent_not_ready: {error}"))?
            .ok_or_else(|| "cloud_agent_not_ready: no deployment recorded".to_owned())
    }
}

/// What one request produced.
struct Answer {
    session: Session,
    outcome: Outcome,
    duration: Duration,
}

impl HorizonApp {
    /// Starts one request from the queue. Its own thread answers the queue, so the route to
    /// the worker and the call on it never run on the UI thread.
    pub(in crate::app) fn start_cloud_agent(&mut self, request: UsageRequest, ctx: &egui::Context) {
        let prepared = Ask::of(&request).and_then(|ask| {
            let route = Route::of(self, &request, ask.cloud(), ask.codes())?;
            Ok((ask, route))
        });
        let ctx = ctx.clone();
        let (ask, route) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                std::thread::spawn(move || {
                    complete(&request, &Err(error));
                    ctx.request_repaint();
                });
                return;
            }
        };
        let actor = request.actor.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = answer(&route, &ask);
            let elapsed = started.elapsed();
            let record = audit(&actor, &ask, result.as_ref().err(), elapsed);
            if let Err(error) = append_cloud_agent_audit(&record) {
                tracing::warn!(kind = ?error.kind(), "could not record a cloud agent call");
            }
            complete(&request, &result);
            ctx.request_repaint();
        });
    }
}

/// Runs one checked request on its route, first proving that its cloud is ready.
fn answer(route: &Route, ask: &Ask) -> Result<Answer, String> {
    route.ready(ask.codes())?;
    let record = Ask::record(route)?;
    let session = agent_session::session(&record, ask.session())
        .cloned()
        .ok_or_else(|| Failure::UnknownSession.to_string())?;
    let target = Target {
        directory: &route.directory,
        settings: &route.settings,
        placement: &route.placement,
    };
    let started = Instant::now();
    let cancel = Cancellation::default();
    let outcome = match ask {
        Ask::Message(message) => agent_session::message(&target, &session, &message.text, &cancel),
        Ask::Read(read) => agent_session::read(&target, &session, read.lines, &cancel),
    }
    .map_err(|failure| failure.to_string())?;
    Ok(Answer {
        session,
        outcome,
        duration: started.elapsed(),
    })
}

/// Records what Horizon asked for and how it ended, never the instruction text.
fn audit(actor: &str, ask: &Ask, error: Option<&String>, duration: Duration) -> CloudAgentAudit {
    let mut record = CloudAgentAudit::queued(actor, ask.cloud(), ask.session(), ask.action(), ask.text_bytes());
    record.duration_millis = Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
    record.code = error.cloned();
    record.outcome = match error {
        Some(_) => CloudAgentOutcome::Failed,
        None => CloudAgentOutcome::Completed,
    };
    record
}

/// Answers the queue with the result of one request. Every caller runs on the request's own
/// thread, never on the UI thread, so the write takes its lock there.
fn complete(request: &UsageRequest, result: &Result<Answer, String>) {
    let mut outcome = request.result(Vec::new(), result.as_ref().err().cloned());
    if let Ok(answer) = result {
        let value = answer_value(answer);
        if request.cloud_agent_message.is_some() {
            outcome.cloud_agent_message = Some(value);
        } else {
            outcome.cloud_agent_read = Some(value);
        }
    }
    if let Err(error) = complete_provider_usage(&outcome) {
        tracing::warn!(kind = ?error.kind(), "could not answer a cloud agent request");
    }
}

fn answer_value(answer: &Answer) -> Value {
    json!({
        "agent": answer.session.agent,
        "branch": answer.session.branch,
        "worktree": answer.session.worktree,
        "text": answer.outcome.text,
        "truncated": answer.outcome.truncated,
        "bytes": answer.outcome.text.len(),
        "exit_status": answer.outcome.exit_status,
        "uid": answer.outcome.uid,
        "duration_millis": u64::try_from(answer.duration.as_millis()).unwrap_or(u64::MAX),
    })
}

#[cfg(all(test, unix))]
mod tests;
