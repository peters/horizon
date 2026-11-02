//! Cloud stop and resume requests of agents. Horizon stops or starts one cloud's worker in
//! the caller's workspace, on a thread this host owns, and only when an agent panel of that
//! same actor created the cloud. A stop or a resume cannot be recalled once its provider
//! request is sent, so an answer says which phase the operation reached and never that a
//! sent request was undone.
use super::{HorizonApp, Runtime, Stage, cloud_runtime};
use crate::app::browser_requests::actor_panel;
use egui::Context;
use horizon_core::browser::manifest::{
    self,
    provider_usage::{CloudLifecycleAction, CloudLifecycleOperation, CloudLifecycleRequest, UsageRequest},
};
use horizon_core::cloud_runtime::{
    Cancellation,
    settings::Settings,
    state::{Creator, Deployment, Store},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

/// A provider power request takes a moment, so a frame during one repaints sooner than an
/// idle board.
const POLL_REPAINT: Duration = Duration::from_millis(500);

const UNAVAILABLE: &str = "cloud_lifecycle_unavailable: requires a Horizon agent panel";
const INVALID: &str = "cloud_lifecycle_invalid_request: operation_id is one lowercase UUID; submit needs action stop or resume and one cloud ID";
const EXPIRED: &str = "cloud_lifecycle_expired: the request expired before Horizon answered it; nothing changed";
const UNKNOWN_CLOUD: &str =
    "cloud_lifecycle_unknown_cloud: no cloud with this ID in your workspace; run cloud_list first";
const AMBIGUOUS: &str = "cloud_lifecycle_unknown_cloud: more than one saved cloud has this ID; use the sidebar";
const UNKNOWN_OPERATION: &str = "cloud_lifecycle_unknown_operation: no operation of yours has this ID";
const BUSY: &str = "cloud_lifecycle_busy: another operation owns this cloud";
const AGENT_BUSY: &str = "cloud_lifecycle_busy: an agent's create operation owns this cloud; read its status first";
const ACTION_BUSY: &str = "cloud_lifecycle_busy: this cloud already has a lifecycle operation; poll its status instead";
const NO_RECORD: &str =
    "cloud_lifecycle_refused: Horizon has no deployment record for this cloud; a person uses its card";
const NOT_AN_AGENTS_CLOUD: &str =
    "cloud_lifecycle_refused: not an agent's cloud; a person stops this cloud on its card";
const NO_WORKER: &str = "cloud_lifecycle_refused: this cloud's record names no worker, so there is nothing to stop or resume; cloud_delete removes it";
const IN_PROGRESS: &str = "an action that cannot be recalled may already be in progress; read its status";

#[derive(Default)]
pub(super) struct State {
    queued: Vec<UsageRequest>,
    /// The operations agents named, by the ID each chose. A lost answer is polled here.
    operations: BTreeMap<String, Operation>,
    channel: Option<(Sender<Message>, Receiver<Message>)>,
}

struct Operation {
    id: String,
    actor: String,
    cloud: String,
    action: CloudLifecycleAction,
    /// The card the cloud shows on, so its own view follows the action.
    card: u32,
    phase: Phase,
    /// The cloud's stage after the action, as its record names it.
    stage: Option<Stage>,
    /// What a stop kept, so a caller knows a resume finds the same files.
    kept: Option<String>,
    /// Why the operation failed.
    error: Option<String>,
    /// Whether the operation's thread reported its outcome.
    finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Horizon accepted the operation and checks this cloud's record.
    Submitted,
    Stopping,
    Stopped,
    Resuming,
    Resumed,
    Failed,
}

impl Phase {
    const fn label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Resuming => "resuming",
            Self::Resumed => "resumed",
            Self::Failed => "failed",
        }
    }

    const fn done(self) -> bool {
        matches!(self, Self::Stopped | Self::Resumed | Self::Failed)
    }
}

/// What an operation's thread reports to the host that owns it.
enum Message {
    /// The provider request is under way.
    Moving { operation: String, phase: Phase },
    /// The action ended. `stage` is the cloud's stage after it, `kept` what a stop kept and
    /// `error` says why it stopped instead.
    Finished {
        operation: String,
        stage: Option<Stage>,
        kept: Option<String>,
        error: Option<String>,
    },
}

impl Message {
    /// The operation this message belongs to.
    fn operation(&self) -> &str {
        match self {
            Self::Moving { operation, .. } | Self::Finished { operation, .. } => operation,
        }
    }
}

impl State {
    fn sender(&mut self) -> Sender<Message> {
        self.channel.get_or_insert_with(channel).0.clone()
    }

    fn messages(&self) -> Vec<Message> {
        self.channel
            .as_ref()
            .map_or_else(Vec::new, |(_, receiver)| receiver.try_iter().collect())
    }

    /// Whether an operation still waits for what its thread reports.
    fn running(&self) -> bool {
        self.operations.values().any(|operation| !operation.finished)
    }

    fn ended(&self, operation: &str) -> bool {
        self.operations
            .get(operation)
            .is_none_or(|running| running.phase.done())
    }
}

impl HorizonApp {
    pub(in crate::app) fn queue_cloud_lifecycle_request(&mut self, request: UsageRequest) {
        self.cloud_prototype.production.lifecycle_agent.queued.push(request);
    }

    /// Answers the queued lifecycle requests and applies what the operations' threads report.
    pub(in crate::app::cloud_panel) fn poll_cloud_lifecycle_requests(&mut self, ctx: &egui::Context) {
        for request in std::mem::take(&mut self.cloud_prototype.production.lifecycle_agent.queued) {
            let answer = self.answer_cloud_lifecycle(&request, ctx);
            self.publish_host_usage_result(answer_result(&request, answer), request.deadline_at_millis);
        }
        for message in self.cloud_prototype.production.lifecycle_agent.messages() {
            self.apply_lifecycle_message(message);
        }
        if self.cloud_prototype.production.lifecycle_agent.running() {
            ctx.request_repaint_after(POLL_REPAINT);
        }
    }

    fn answer_cloud_lifecycle(&mut self, request: &UsageRequest, ctx: &Context) -> Result<Value, String> {
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or(UNAVAILABLE)?;
        let lifecycle = request
            .cloud_lifecycle
            .as_ref()
            .filter(|lifecycle| lifecycle.valid())
            .ok_or(INVALID)?;
        match lifecycle.operation {
            CloudLifecycleOperation::Status => self.lifecycle_status(&request.actor, lifecycle),
            CloudLifecycleOperation::Submit => {
                // A caller that stopped waiting gets no action it did not see.
                if manifest::now_millis() >= request.deadline_at_millis {
                    return Err(EXPIRED.into());
                }
                self.submit_lifecycle(&request.actor, &workspace, lifecycle, ctx)
            }
        }
    }

    fn submit_lifecycle(
        &mut self,
        actor: &str,
        workspace: &str,
        lifecycle: &CloudLifecycleRequest,
        ctx: &Context,
    ) -> Result<Value, String> {
        let id = lifecycle.operation_id().to_owned();
        if let Some(known) = self.cloud_prototype.production.lifecycle_agent.operations.get(&id) {
            if known.actor != actor {
                return Err(UNKNOWN_OPERATION.into());
            }
            // The answer was lost and the caller sent the same operation again: no second
            // provider request of the same action.
            return Ok(status(known));
        }
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("cloud_lifecycle_unavailable: Horizon has no cloud state directory")?;
        let wanted = lifecycle.cloud.as_deref().unwrap_or_default();
        let index = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .enumerate()
            .filter(|(_, group)| group.workspace == workspace)
            .find(|(_, group)| group.remote.as_ref().is_some_and(|remote| remote.id == wanted))
            .map(|(index, _)| index)
            .ok_or(UNKNOWN_CLOUD)?;
        // Two saved clouds with one ID, in any workspace, make the ID ambiguous.
        let same_id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == wanted))
            .count();
        if same_id != 1 {
            return Err(AMBIGUOUS.into());
        }
        let card = self.cloud_prototype.groups.0[index].issue;
        // The cloud's own operation, its provider check and an agent's create own it first.
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&card)
            .is_some_and(Runtime::busy)
        {
            return Err(BUSY.into());
        }
        if self.cloud_prototype.production.create_agent.owns_cloud(wanted) {
            return Err(AGENT_BUSY.into());
        }
        if self
            .cloud_prototype
            .production
            .lifecycle_agent
            .operations
            .values()
            .any(|running| running.cloud == wanted && !running.finished)
        {
            return Err(ACTION_BUSY.into());
        }
        let action = lifecycle.action.ok_or(INVALID)?;
        let cancel = Cancellation::default();
        let state = &mut self.cloud_prototype.production.lifecycle_agent;
        state.operations.insert(
            id.clone(),
            Operation {
                id: id.clone(),
                actor: actor.into(),
                cloud: wanted.into(),
                action,
                card,
                phase: Phase::Submitted,
                stage: None,
                kept: None,
                error: None,
                finished: false,
            },
        );
        let sender = state.sender();
        let ctx = ctx.clone();
        let (thread_id, thread_cloud, thread_actor) = (id.clone(), wanted.to_owned(), actor.to_owned());
        // The cloud's record, its credentials and the provider request run off this thread,
        // so the frame that answers the caller never waits for them.
        std::thread::spawn(move || {
            run_lifecycle(
                &root,
                (&thread_cloud, &thread_actor),
                (&thread_id, action, &cancel),
                (&sender, &ctx),
            );
        });
        Ok(json!({
            "operation_id": id,
            "phase": Phase::Submitted.label(),
            "cloud": wanted,
            "action": action_label(action),
            "done": false,
            "in_progress": IN_PROGRESS,
        }))
    }

    fn lifecycle_status(&self, actor: &str, lifecycle: &CloudLifecycleRequest) -> Result<Value, String> {
        let known = self
            .cloud_prototype
            .production
            .lifecycle_agent
            .operations
            .get(lifecycle.operation_id())
            .filter(|known| known.actor == actor)
            .ok_or(UNKNOWN_OPERATION)?;
        Ok(status(known))
    }

    fn apply_lifecycle_message(&mut self, message: Message) {
        let operation = message.operation().to_owned();
        let ended = self.cloud_prototype.production.lifecycle_agent.ended(&operation);
        let Some(running) = self
            .cloud_prototype
            .production
            .lifecycle_agent
            .operations
            .get_mut(&operation)
        else {
            return;
        };
        match message {
            Message::Moving { phase, .. } if !ended => running.phase = phase,
            Message::Moving { .. } => {}
            Message::Finished { stage, kept, error, .. } => {
                running.finished = true;
                running.stage = stage;
                running.kept = kept;
                running.error = error;
                running.phase = match (&running.error, running.action) {
                    (Some(_), _) => Phase::Failed,
                    (None, CloudLifecycleAction::Stop) => Phase::Stopped,
                    (None, CloudLifecycleAction::Resume) => Phase::Resumed,
                };
                // The card's own view follows the record, as it does for the card's actions.
                let card = running.card;
                let state = running.stage;
                if let Some((runtime, stage)) = self.cloud_prototype.production.runtimes.get_mut(&card).zip(state) {
                    runtime.stage = Some(stage);
                }
            }
        }
    }
}

/// One operation's answer, for its submission and for every later poll.
fn status(operation: &Operation) -> Value {
    json!({
        "operation_id": operation.id,
        "cloud": operation.cloud,
        "action": action_label(operation.action),
        "phase": operation.phase.label(),
        "done": operation.phase.done(),
        "stage": operation.stage.map(Stage::label),
        "kept": operation.kept,
        "error": operation.error,
    })
}

const fn action_label(action: CloudLifecycleAction) -> &'static str {
    match action {
        CloudLifecycleAction::Stop => "stop",
        CloudLifecycleAction::Resume => "resume",
    }
}

fn answer_result(
    request: &UsageRequest,
    answer: Result<Value, String>,
) -> horizon_core::browser::manifest::provider_usage::UsageResult {
    match answer {
        Ok(value) => {
            let mut result = request.result(Vec::new(), None);
            result.cloud_lifecycle = Some(value);
            result
        }
        Err(error) => request.result(Vec::new(), Some(error)),
    }
}

/// The action one operation runs: its record, its credentials and its provider request all
/// leave this thread.
fn run_lifecycle(
    root: &Path,
    (cloud, actor): (&str, &str),
    (operation, action, cancel): (&str, CloudLifecycleAction, &Cancellation),
    (sender, ctx): (&Sender<Message>, &Context),
) {
    let send = |message: Message| {
        let _ = sender.send(message);
        ctx.request_repaint();
    };
    let finished = |stage: Option<Stage>, kept: Option<String>, error: Option<String>| Message::Finished {
        operation: operation.to_owned(),
        stage,
        kept,
        error,
    };
    let state_root = match cloud_runtime::state::cloud_directory(root, cloud) {
        Ok(state_root) => state_root,
        Err(error) => {
            send(finished(None, None, Some(failed(action, error))));
            return;
        }
    };
    let (record, settings) = match admitted(&state_root, root, actor) {
        Ok(admitted) => admitted,
        Err(error) => {
            send(finished(None, None, Some(error)));
            return;
        }
    };
    if record.spec.is_none() || record.worker.is_none() {
        send(finished(Some(record.stage), None, Some(NO_WORKER.to_owned())));
        return;
    }
    let kept = kept_by(&record);
    match action {
        CloudLifecycleAction::Stop => {
            if record.stage == Stage::Stopped {
                // Nothing to send: the cloud is already stopped, and a second stop would
                // only ask the provider what it already answered.
                send(finished(Some(Stage::Stopped), Some(kept), None));
                return;
            }
            send(Message::Moving {
                operation: operation.to_owned(),
                phase: Phase::Stopping,
            });
            match cloud_runtime::lifecycle::stop(&state_root, &settings, cancel) {
                Ok(state) => send(finished(Some(state.stage), Some(kept), None)),
                Err(error) => send(finished(None, None, Some(failed(action, error)))),
            }
        }
        CloudLifecycleAction::Resume => {
            if record.stage != Stage::Stopped {
                send(finished(
                    Some(record.stage),
                    None,
                    Some(
                        "cloud_lifecycle_refused: this cloud is not stopped, so there is nothing to resume; read cloud_list for its stage"
                            .to_owned(),
                    ),
                ));
                return;
            }
            send(Message::Moving {
                operation: operation.to_owned(),
                phase: Phase::Resuming,
            });
            match cloud_runtime::lifecycle::resume(&state_root, &settings, cancel) {
                Ok(()) => match Store::snapshot(&state_root) {
                    Ok(state) => send(finished(state.map(|state| state.stage), None, None)),
                    Err(error) => send(finished(None, None, Some(failed(action, error)))),
                },
                Err(error) => send(finished(None, None, Some(failed(action, error)))),
            }
        }
    }
}

/// The cloud's record and the machine's cloud settings, or why this caller may not act.
fn admitted(state_root: &Path, root: &Path, actor: &str) -> Result<(Deployment, Settings), String> {
    let store = Store::lock_existing(state_root).map_err(|error| match error {
        cloud_runtime::Error::Io(error) if error.kind() == std::io::ErrorKind::NotFound => NO_RECORD.to_owned(),
        cloud_runtime::Error::Busy => BUSY.to_owned(),
        error => format!("cloud_lifecycle_failed: {error}"),
    })?;
    let record = store
        .load()
        .map_err(|error| format!("cloud_lifecycle_failed: {error}"))?
        .ok_or(NO_RECORD)?;
    drop(store);
    match &record.created_by {
        Some(Creator { actor: creator, .. }) if creator == actor => {}
        _ => return Err(NOT_AN_AGENTS_CLOUD.to_owned()),
    }
    let settings = Settings::load(&root.join("settings.json"))
        .map_err(|error| format!("cloud_lifecycle_failed: the cloud settings could not be read: {error}"))?;
    Ok((record, settings))
}

/// What a stop of this cloud keeps, so a caller knows a resume finds the same files.
fn kept_by(record: &Deployment) -> String {
    let kept = horizon_core::cloud_runtime::provider::Description::of(&record.profile).stopped;
    match kept {
        horizon_core::cloud_runtime::provider::StoppedCost::WorkerKept => {
            "the worker and its workspace, billed at the stopped rate"
        }
        horizon_core::cloud_runtime::provider::StoppedCost::ServerDeleted => {
            "the workspace volume; a resume creates a new server on it"
        }
    }
    .to_owned()
}

fn failed(action: CloudLifecycleAction, error: cloud_runtime::Error) -> String {
    match error {
        cloud_runtime::Error::Busy => BUSY.to_owned(),
        error => format!(
            "cloud_lifecycle_failed: {} did not finish: {error}",
            action_label(action)
        ),
    }
}

#[cfg(all(test, unix))]
mod tests;
