//! Agent requests to run one bounded command in a cloud worker's agent account. Horizon
//! resolves the cloud through the board's own groups, refuses a cloud that is not ready or
//! that the caller may not use, and answers from a background thread: the route, the
//! settings and the command never run on the UI thread.
use super::HorizonApp;
use crate::app::browser_requests::actor_panel;
use horizon_core::{
    browser::manifest::{
        self,
        provider_usage::{
            CloudExecAudit, CloudExecOutcome, CloudExecRequest, UsageRequest, append_cloud_exec_audit,
            complete_provider_usage,
        },
    },
    cloud_panel::Placement,
    cloud_runtime::{
        self, Cancellation, Stage,
        cloud_exec::{self, Command, Failure, Target},
        state::Store,
    },
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// The route one request takes: a cloud's own record, which holds the host key Horizon
/// pinned for its worker.
#[derive(Debug)]
struct Route {
    directory: PathBuf,
    settings: PathBuf,
    placement: Placement,
}

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
            .ok_or("cloud_exec_invalid_request: one command for one cloud ID from cloud_list")?;
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or("cloud_exec_unavailable: requires a Horizon agent panel")?;
        // Two saved clouds with one ID, in any workspace, make the ID ambiguous.
        let same_id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == exec.cloud))
            .count();
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.workspace == workspace && group.remote.as_ref().is_some_and(|r| r.id == exec.cloud))
            .ok_or("cloud_exec_unknown_cloud: no cloud with this ID in your workspace; read cloud_list first")?;
        if same_id != 1 {
            return Err("cloud_exec_ambiguous_cloud: more than one cloud has this ID; use the sidebar".into());
        }
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("cloud_exec_not_ready: Horizon has no cloud state directory")?;
        // A caller that stopped waiting gets nothing that runs.
        if manifest::now_millis() >= request.deadline_at_millis {
            return Err(
                "cloud_exec_expired: the request expired before Horizon started the command; nothing ran".into(),
            );
        }
        let directory = cloud_runtime::state::cloud_directory(&root, &exec.cloud).map_err(|error| error.to_string())?;
        let placement = group
            .remote
            .as_ref()
            .map(|launch| launch.placement.clone())
            .unwrap_or_default();
        Ok(Route {
            directory,
            settings: root.join("settings.json"),
            placement,
        })
    }

    /// The command to run, refused here when it names another account.
    fn cloud_exec_command(request: &UsageRequest) -> Result<Command, String> {
        let exec = request
            .cloud_exec
            .as_ref()
            .filter(|exec| exec.valid())
            .ok_or("cloud_exec_invalid_request: one command for one cloud ID from cloud_list")?;
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

/// Whether the cloud's own record says that its worker is ready. A command changes a
/// running worker's checkout, so a cloud that is deploying, stopped or deleted takes none.
fn ready(route: &Route) -> Result<(), String> {
    // A removed folder is not made again: an exec creates nothing on the cloud's disk.
    let Ok(store) = Store::lock_existing(&route.directory) else {
        return Err("cloud_exec_not_ready: no deployment recorded".into());
    };
    let stage = store
        .load()
        .map_err(|error| format!("cloud_exec_not_ready: {error}"))?
        .map(|state| state.stage);
    match stage {
        Some(Stage::Ready) => Ok(()),
        Some(stage) => Err(format!("cloud_exec_not_ready: {}", stage.label())),
        None => Err("cloud_exec_not_ready: no deployment recorded".into()),
    }
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
