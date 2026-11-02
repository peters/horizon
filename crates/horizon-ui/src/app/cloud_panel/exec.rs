//! Agent requests to run one bounded command in a cloud worker's agent account. Horizon
//! resolves the cloud through the board's own groups, refuses a cloud that is not ready or
//! that the caller may not use, and answers from a background thread: the route, the
//! settings and the command never run on the UI thread.
use super::HorizonApp;
use super::route::{Codes, Route};
use horizon_core::{
    browser::manifest::provider_usage::{
        CloudExecAudit, CloudExecOutcome, CloudExecRequest, UsageRequest, append_cloud_exec_audit,
        complete_provider_usage,
    },
    cloud_runtime::{
        Cancellation,
        cloud_exec::{self, Command, Failure, Target},
    },
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The codes and sentences this tool uses for its own refusals.
const CODES: Codes = Codes {
    prefix: "cloud_exec",
    request: "one command for one cloud ID from cloud_list",
    started: "the command",
};

/// What one request produced. A non-zero exit status is an answer, not a failure.
#[derive(Debug)]
enum Outcome {
    Ran(cloud_exec::Answer),
    TimedOut,
    Failed(String),
}

impl Outcome {
    fn error(&self) -> Option<String> {
        match self {
            Self::Ran(_) => None,
            Self::TimedOut => Some(Failure::TimedOut.to_string()),
            Self::Failed(error) => Some(error.clone()),
        }
    }
}

impl HorizonApp {
    /// Starts one command from the queue. It answers the queue itself from its own thread,
    /// so neither the route to the worker nor the command runs on the UI thread.
    pub(in crate::app) fn start_cloud_exec(&mut self, request: UsageRequest, ctx: &egui::Context) {
        let (route, command) = match (self.cloud_exec_route(&request), Self::cloud_exec_command(&request)) {
            (Ok(route), Ok(command)) => (route, command),
            (Err(error), _) | (_, Err(error)) => {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    complete(&request, &Outcome::Failed(error));
                    ctx.request_repaint();
                });
                return;
            }
        };
        let exec = request.cloud_exec.clone();
        let actor = request.actor.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let outcome = execute(&route, &command);
            if let Some(exec) = &exec {
                let record = audit(&actor, exec, &outcome, started.elapsed());
                if let Err(error) = append_cloud_exec_audit(&record) {
                    tracing::warn!(kind = ?error.kind(), "could not record a cloud command");
                }
            }
            complete(&request, &outcome);
            ctx.request_repaint();
        });
    }

    /// The cloud the caller may command, with the route to its worker.
    fn cloud_exec_route(&self, request: &UsageRequest) -> Result<Route, String> {
        let exec = request
            .cloud_exec
            .as_ref()
            .filter(|exec| exec.valid())
            .ok_or_else(invalid)?;
        Route::of(self, request, &exec.cloud, &CODES)
    }

    /// The command to run, refused here when it names another account.
    fn cloud_exec_command(request: &UsageRequest) -> Result<Command, String> {
        let exec = request
            .cloud_exec
            .as_ref()
            .filter(|exec| exec.valid())
            .ok_or_else(invalid)?;
        let command = Command {
            text: exec.command.clone(),
            timeout: Duration::from_millis(exec.timeout_millis),
        };
        cloud_exec::check(&command).map_err(|refusal| refusal.to_string())?;
        Ok(command)
    }
}

/// Runs one checked command on its route, first proving that its cloud is ready.
fn execute(route: &Route, command: &Command) -> Outcome {
    if let Err(error) = ready(route) {
        return Outcome::Failed(error);
    }
    let target = Target {
        directory: &route.directory,
        settings: &route.settings,
        placement: &route.placement,
    };
    match cloud_exec::execute(&target, command, &Cancellation::default()) {
        Ok(answer) => Outcome::Ran(answer),
        Err(Failure::TimedOut) => Outcome::TimedOut,
        Err(failure) => Outcome::Failed(failure.to_string()),
    }
}

/// Whether the cloud's own record says that its worker is ready.
fn ready(route: &Route) -> Result<(), String> {
    route.ready(&CODES)
}

/// The refusal of a request that does not carry one command for one cloud.
fn invalid() -> String {
    format!("cloud_exec_invalid_request: {}", CODES.request)
}

/// Records what Horizon asked for and how it ended, never the command text.
fn audit(actor: &str, exec: &CloudExecRequest, outcome: &Outcome, duration: Duration) -> CloudExecAudit {
    let mut record = CloudExecAudit::queued(actor, exec);
    record.duration_millis = Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
    record.code = outcome.error();
    match outcome {
        Outcome::Ran(answer) => {
            record.outcome = CloudExecOutcome::Completed;
            record.exit_status = Some(answer.exit_status);
            record.uid = Some(answer.uid);
        }
        Outcome::TimedOut => record.outcome = CloudExecOutcome::TimedOut,
        Outcome::Failed(_) => record.outcome = CloudExecOutcome::Failed,
    }
    record
}

/// Answers the queue with the result of one request. Every caller runs on the
/// request's own thread, never on the UI thread, so the write takes its lock
/// there.
fn complete(request: &UsageRequest, outcome: &Outcome) {
    let mut result = request.result(Vec::new(), outcome.error());
    if let Outcome::Ran(answer) = outcome {
        result.cloud_exec = Some(answer_value(answer));
    }
    if let Err(error) = complete_provider_usage(&result) {
        tracing::warn!(kind = ?error.kind(), "could not answer a cloud command request");
    }
}

fn answer_value(answer: &cloud_exec::Answer) -> Value {
    json!({
        "exit_status": answer.exit_status,
        "uid": answer.uid,
        "stdout": answer.stdout,
        "stderr": answer.stderr,
        "duration_millis": u64::try_from(answer.duration.as_millis()).unwrap_or(u64::MAX),
    })
}

#[cfg(all(test, unix))]
mod tests;
